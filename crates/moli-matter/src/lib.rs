//! Matter driver, as a client of a Matter server.
//!
//! The server (python-matter-server, or the matter.js one) owns the fabric
//! and the sessions with the devices; Moli only listens to it over its
//! websocket: `start_listening` returns every node with its attributes
//! (`"endpoint/cluster/attribute"` → value), then `attribute_updated`,
//! `node_updated` and `node_removed` events follow. No Matter session of
//! Moli's own: a lamp that already has a controller gets no extra load.
//!
//! Commands go through the server's `device_command`. `writable` limits which
//! nodes Moli may command (absent: every node). The colour-temperature range
//! is a per-model profile (`models`): some lamps announce a range they do not
//! have, and outside the real one they go nearly dark.

mod map;

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context as _, bail, ensure};
use moli_core::DeviceId;
use moli_net::ws::WebSocket;
use moli_runtime::{BoxFuture, CommandRequest, Driver, DriverCtx};
use serde::Deserialize;
use serde_json::{Value as Json, json};

/// Driver configuration (`[driver.options]` in `moli.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Node numbers Moli may command. Absent: every node. Empty: none.
    #[serde(default)]
    pub writable: Option<Vec<u64>>,
    /// Colour temperature range in kelvin, for models without a profile
    /// (default 2700-6500, what the common Govee lamps really do).
    #[serde(default = "default_kelvin")]
    pub color_temp_k: [u32; 2],
    /// Per product name (Matter `ProductName`, e.g. `H61E6`): its own profile.
    #[serde(default)]
    pub models: HashMap<String, Model>,
}

/// What one lamp model really does, as the maker documents it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub color_temp_k: [u32; 2],
}

fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    5580
}
fn default_kelvin() -> [u32; 2] {
    [2700, 6500]
}

impl Config {
    /// The range in mireds (warmest kelvin = most mireds) for a model. The warm
    /// bound rounds down, the cold one to the nearest mired (one mired is ~80 K
    /// at 9000 K: rounding inward would cost the lamp its last 1000 K).
    fn mireds(&self, model: Option<&str>) -> [u32; 2] {
        let [warm, cold] = model
            .and_then(|m| self.models.get(m))
            .map_or(self.color_temp_k, |m| m.color_temp_k);
        [(1_000_000 + cold / 2) / cold, 1_000_000 / warm]
    }

    fn check(&self) -> anyhow::Result<()> {
        for (what, [warm, cold]) in std::iter::once(("color_temp_k", self.color_temp_k)).chain(
            self.models
                .iter()
                .map(|(m, p)| (m.as_str(), p.color_temp_k)),
        ) {
            ensure!(
                warm >= 1000 && warm < cold,
                "{what}: [warmest, coldest] in kelvin, e.g. [2700, 6500]"
            );
        }
        Ok(())
    }

    fn may_command(&self, node: u64) -> bool {
        self.writable
            .as_ref()
            .is_none_or(|nodes| nodes.contains(&node))
    }
}

#[derive(Debug)]
pub struct Matter {
    config: Config,
}

impl Matter {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Driver for Matter {
    fn kind(&self) -> &'static str {
        "matter"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// A quiet server is pinged (a dead link fails the write; the pong itself is
/// swallowed by the websocket, so silence alone proves nothing).
const QUIET: Duration = Duration::from_secs(30);

/// Oldest schema this driver understands (the messages used here are stable
/// since 11).
const MIN_SCHEMA: i64 = 11;

/// Answer to a command on a node outside `writable`.
const READ_ONLY: &str = "Matter : appareil en lecture seule (options.writable)";

/// What the loop keeps between messages.
#[derive(Default)]
struct Session {
    /// Commands sent and not yet answered, by message id.
    pending: HashMap<String, CommandRequest>,
    /// The colour state per node: a `color` / `color_temp` reading needs the
    /// mode and the values of both.
    colors: HashMap<u64, map::Color>,
    next: u64,
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    config.check()?;
    let mut ws = WebSocket::connect_plain(&config.host, config.port, "/ws", &[])
        .await
        .context("matter server")?;
    ws.send_text(&json!({"message_id": "start", "command": "start_listening"}).to_string())
        .await?;
    let mut session = Session::default();
    loop {
        tokio::select! {
            command = ctx.next_command() => match command {
                Some(command) => send(config, &mut ws, &mut session, command).await?,
                None => return Ok(()),
            },
            text = ws.recv_text(QUIET) => match text {
                Ok(text) => on_message(ctx, config, &mut session, &text)?,
                Err(e) if e.to_string().contains("in time") => ws.ping().await?,
                Err(e) => return Err(e),
            },
        }
    }
}

/// Turns a command into `device_command`; the answer comes back by message id.
async fn send(
    config: &Config,
    ws: &mut WebSocket,
    session: &mut Session,
    command: CommandRequest,
) -> anyhow::Result<()> {
    let device = command.device.id.to_string();
    let Some(node) = device
        .rsplit(':')
        .next()
        .and_then(|n| n.parse::<u64>().ok())
    else {
        command.reply(Err("not a matter node".into()));
        return Ok(());
    };
    if !config.may_command(node) {
        command.reply(Err(READ_ONLY.into()));
        return Ok(());
    }
    let mireds = config.mireds(command.device.model.as_deref());
    match map::encode(&command.key, &command.value, mireds) {
        Ok(order) => {
            session.pending.retain(|_, c| !c.abandoned());
            session.next += 1;
            let id = format!("c{}", session.next);
            let request = json!({"message_id": id, "command": "device_command", "args": {
                "node_id": node, "endpoint_id": order.endpoint, "cluster_id": order.cluster,
                "command_name": order.name, "payload": order.payload}});
            ws.send_text(&request.to_string()).await?;
            session.pending.insert(id, command);
        }
        Err(e) => command.reply(Err(e)),
    }
    Ok(())
}

fn on_message(
    ctx: &DriverCtx,
    config: &Config,
    session: &mut Session,
    text: &str,
) -> anyhow::Result<()> {
    let message: Json = serde_json::from_str(text).context("matter server: not JSON")?;
    if let Some(version) = message["schema_version"].as_i64() {
        tracing::info!(schema = version, sdk = %message["sdk_version"], "matter server");
        if version < MIN_SCHEMA {
            bail!("matter server schema {version} is older than {MIN_SCHEMA}");
        }
    } else if let Some(command) = message["message_id"]
        .as_str()
        .and_then(|id| session.pending.remove(id))
    {
        command.reply(if message.get("error_code").is_some() {
            Err(format!(
                "matter : {}",
                message["details"].as_str().unwrap_or("refusé")
            ))
        } else {
            Ok(())
        });
    } else if message["message_id"] == "start" {
        let Some(nodes) = message["result"].as_array() else {
            bail!("start_listening refused: {}", message["details"]);
        };
        let known = ctx.devices();
        let seen: Vec<DeviceId> = nodes
            .iter()
            .filter_map(|node| apply_node(ctx, config, session, node))
            .collect();
        for id in known.into_iter().filter(|id| !seen.contains(id)) {
            ctx.remove_device(&id);
        }
        tracing::info!(devices = seen.len(), "matter nodes loaded");
        ctx.ready();
    } else if let Some(event) = message["event"].as_str() {
        on_event(ctx, config, session, event, &message["data"]);
    }
    Ok(())
}

fn on_event(ctx: &DriverCtx, config: &Config, session: &mut Session, event: &str, data: &Json) {
    match event {
        "node_added" | "node_updated" => {
            apply_node(ctx, config, session, data);
        }
        "node_removed" => {
            if let Some(node) = data.as_u64() {
                session.colors.remove(&node);
                ctx.remove_device(&ctx.device_id(&node.to_string()));
            }
        }
        "attribute_updated" => {
            if let (Some(node), Some(path)) = (data[0].as_u64(), data[1].as_str()) {
                let device = ctx.device_id(&node.to_string());
                let color = session.colors.entry(node).or_default();
                if color.update(path, &data[2]) {
                    for (key, value) in color.states() {
                        ctx.set_state(&device, key, value);
                    }
                } else if let Some((key, value)) = map::decode(path, &data[2]) {
                    ctx.set_state(&device, &key, value);
                }
            }
        }
        _ => {}
    }
}

/// Publishes a node as a device with its current values. `None` for a node
/// with nothing Moli models (a bridge, a coordinator).
fn apply_node(
    ctx: &DriverCtx,
    config: &Config,
    session: &mut Session,
    node: &Json,
) -> Option<DeviceId> {
    let number = node["node_id"].as_u64()?;
    let model = node["attributes"]["0/40/3"].as_str();
    let (device, states, available) = map::node(
        ctx.instance(),
        node,
        config.may_command(number),
        config.mireds(model),
    )?;
    if let Some(attributes) = node["attributes"].as_object() {
        session
            .colors
            .insert(number, map::Color::from_attributes(attributes));
    }
    let id = device.id.clone();
    ctx.upsert_device(device);
    for (key, value) in states {
        ctx.set_state(&id, &key, value);
    }
    ctx.set_availability(&id, available);
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> Config {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn config_defaults() {
        let config = config("{}");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 5580);
        assert!(config.writable.is_none());
        assert_eq!(config.mireds(Some("H60C1")), [154, 370], "2700-6500 K");
        config.check().unwrap();
    }

    #[test]
    fn a_model_profile_wins() {
        let config = config(r#"{"models": {"H61E6": {"color_temp_k": [2000, 9900]}}}"#);
        assert_eq!(config.mireds(Some("H61E6")), [101, 500]);
        assert_eq!(config.mireds(Some("H60C1")), [154, 370]);
        assert_eq!(config.mireds(None), [154, 370]);
    }

    #[test]
    fn a_bad_range_is_refused() {
        assert!(config(r#"{"color_temp_k": [6500, 2700]}"#).check().is_err());
        assert!(
            config(r#"{"models": {"X": {"color_temp_k": [0, 100]}}}"#)
                .check()
                .is_err()
        );
    }

    #[test]
    fn writable_limits_commands() {
        let config = config(r#"{"writable": [2]}"#);
        assert!(config.may_command(2) && !config.may_command(3));
    }
}
