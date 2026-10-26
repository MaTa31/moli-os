//! Moli's own vocabulary for what Z2M says its own way.
//!
//! The dashboard's shutter and air-conditioner cards speak `control` /
//! `percent_control` and `on` / `mode` / `target_temperature` /
//! `temperature` (what the Tuya and Daikin drivers publish). A Zigbee shutter
//! says `state` (OPEN/CLOSE/STOP) and `position`; a Zigbee air-conditioner
//! remote says `system_mode`, two setpoints and `local_temperature`. This
//! module adds the Moli points next to the Z2M ones and converts both ways.

use std::sync::Arc;

use moli_core::{Access, Kind, PointSpec, Semantic, Unit, Value};
use moli_i18n::tr;
use serde_json::{Map, Value as Json, json};

/// What a Z2M device is, when Moli has a card for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Adapter {
    #[default]
    None,
    Cover,
    Climate,
}

/// Moli's mode names (those of the Daikin unit) and Z2M's `system_mode`.
const MODES: [(&str, &str); 5] = [
    ("auto", "auto"),
    ("déshumidification", "dry"),
    ("froid", "cool"),
    ("chaud", "heat"),
    ("ventilation", "fan_only"),
];

fn to_z2m(mode: &str) -> Option<&'static str> {
    MODES.iter().find(|(m, _)| *m == mode).map(|(_, z)| *z)
}

fn from_z2m(mode: &str) -> Option<&'static str> {
    MODES.iter().find(|(_, z)| *z == mode).map(|(m, _)| *m)
}

pub fn detect(points: &[PointSpec]) -> Adapter {
    let has = |key: &str| points.iter().any(|p| &*p.key == key);
    let opens = points.iter().any(|p| {
        &*p.key == "state"
            && matches!(&p.kind, Kind::Enum { values } if values.iter().any(|v| &**v == "OPEN"))
    });
    if opens && has("position") {
        Adapter::Cover
    } else if has("system_mode") && has("local_temperature") {
        Adapter::Climate
    } else {
        Adapter::None
    }
}

fn point(
    key: &str,
    label: String,
    kind: Kind,
    write: bool,
    unit: Option<Unit>,
    semantic: Semantic,
) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access { read: true, write },
        unit,
        semantic,
    }
}

/// The Moli points to add next to the Z2M ones.
pub fn points(adapter: Adapter) -> Vec<PointSpec> {
    match adapter {
        Adapter::None => Vec::new(),
        Adapter::Cover => vec![
            point(
                "control",
                tr!("pilotes.z2m.commande"),
                Kind::Enum {
                    values: ["open", "stop", "close"].map(Arc::from).to_vec(),
                },
                true,
                None,
                // Not `Semantic::Control` (an order to a machine at work, held for a
                // human): a shutter is an ordinary command.
                Semantic::Other,
            ),
            point(
                "percent_control",
                tr!("pilotes.z2m.ouverture"),
                Kind::Numeric {
                    min: Some(0.0),
                    max: Some(100.0),
                    step: None,
                },
                true,
                Some(Unit::Percent),
                Semantic::Other,
            ),
        ],
        Adapter::Climate => vec![
            point(
                "on",
                tr!("pilotes.z2m.marche"),
                Kind::Binary,
                true,
                None,
                Semantic::OnOff,
            ),
            point(
                "mode",
                tr!("pilotes.z2m.mode"),
                Kind::Enum {
                    values: MODES.map(|(m, _)| Arc::from(m)).to_vec(),
                },
                true,
                None,
                Semantic::Config,
            ),
            point(
                "target_temperature",
                tr!("pilotes.z2m.consigne"),
                Kind::Numeric {
                    min: Some(16.0),
                    max: Some(30.0),
                    step: Some(0.5),
                },
                true,
                Some(Unit::Celsius),
                Semantic::Config,
            ),
            point(
                "temperature",
                tr!("pilotes.z2m.temperature"),
                Kind::Numeric {
                    min: None,
                    max: None,
                    step: None,
                },
                false,
                Some(Unit::Celsius),
                Semantic::Temperature,
            ),
        ],
    }
}

fn text<'a>(payload: &'a Map<String, Json>, key: &str) -> Option<&'a str> {
    payload.get(key)?.as_str()
}

/// The Moli points a Z2M state message carries.
pub fn decode(adapter: Adapter, payload: &Map<String, Json>) -> Vec<(Arc<str>, Value)> {
    let mut out: Vec<(Arc<str>, Value)> = Vec::new();
    let mut put = |key: &str, value: Value| out.push((key.into(), value));
    match adapter {
        Adapter::None => {}
        Adapter::Cover => {
            if let Some(state) = text(payload, "state") {
                put("control", Value::Text(state.to_lowercase().into()));
            }
            if let Some(position) = payload.get("position").filter(|p| p.is_number()) {
                put("percent_control", Value::from_json(position));
            }
        }
        Adapter::Climate => {
            let mode = text(payload, "system_mode");
            if let Some(mode) = mode {
                put("on", Value::Bool(mode != "off"));
                if let Some(name) = from_z2m(mode) {
                    put("mode", Value::Text(name.into()));
                }
            }
            if let Some(t) = payload.get("local_temperature").filter(|t| t.is_number()) {
                put("temperature", Value::from_json(t));
            }
            let setpoint = if mode == Some("heat") {
                "occupied_heating_setpoint"
            } else {
                "occupied_cooling_setpoint"
            };
            if let Some(target) = payload.get(setpoint).filter(|t| t.is_number()) {
                put("target_temperature", Value::from_json(target));
            }
        }
    }
    out
}

/// A write on a Moli point → the Z2M `set` payload. `None`: not one of
/// ours, the plain encoding applies. `mode` is the unit's last known Moli mode.
pub fn encode(
    adapter: Adapter,
    key: &str,
    value: &Value,
    mode: Option<&str>,
) -> Option<Result<Json, String>> {
    let number = || match value {
        #[allow(clippy::cast_precision_loss)]
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    };
    let wanted = |m: &str| to_z2m(m).ok_or_else(|| format!("mode inconnu : {m}"));
    match (adapter, key, value) {
        (Adapter::Cover, "control", Value::Text(t)) => {
            Some(Ok(json!({ "state": t.to_uppercase() })))
        }
        (Adapter::Cover, "percent_control", _) => Some(
            number()
                .map(|p| json!({ "position": p.round().clamp(0.0, 100.0) }))
                .ok_or_else(|| "percent_control: a number is expected".to_owned()),
        ),
        (Adapter::Climate, "on", Value::Bool(false)) => Some(Ok(json!({ "system_mode": "off" }))),
        (Adapter::Climate, "on", Value::Bool(true)) => {
            Some(wanted(mode.unwrap_or("froid")).map(|z| json!({ "system_mode": z })))
        }
        (Adapter::Climate, "mode", Value::Text(m)) => {
            Some(wanted(m).map(|z| json!({ "system_mode": z })))
        }
        (Adapter::Climate, "target_temperature", _) => {
            let setpoint = if mode == Some("chaud") {
                "occupied_heating_setpoint"
            } else {
                "occupied_cooling_setpoint"
            };
            Some(
                number()
                    .map(|t| json!({ setpoint: (t * 2.0).round() / 2.0 }))
                    .ok_or_else(|| "target_temperature: a number is expected".to_owned()),
            )
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(v: &Json) -> &Map<String, Json> {
        v.as_object().unwrap()
    }

    fn get(states: &[(Arc<str>, Value)], key: &str) -> Option<Value> {
        states
            .iter()
            .find(|(k, _)| &**k == key)
            .map(|(_, v)| v.clone())
    }

    #[test]
    fn shutters_speak_moli() {
        let states = decode(
            Adapter::Cover,
            payload(&json!({"state": "OPEN", "position": 100})),
        );
        assert_eq!(get(&states, "control"), Some(Value::Text("open".into())));
        assert_eq!(get(&states, "percent_control"), Some(Value::Int(100)));
        let close = encode(
            Adapter::Cover,
            "control",
            &Value::Text("close".into()),
            None,
        );
        assert_eq!(close, Some(Ok(json!({"state": "CLOSE"}))));
        let half = encode(Adapter::Cover, "percent_control", &Value::Float(49.6), None);
        assert_eq!(half, Some(Ok(json!({"position": 50.0}))));
    }

    #[test]
    fn a_unit_in_cool_mode_reads_the_cooling_setpoint() {
        let message = json!({"system_mode": "cool", "local_temperature": 29.4,
            "occupied_cooling_setpoint": 24, "occupied_heating_setpoint": 20});
        let states = decode(Adapter::Climate, payload(&message));
        assert_eq!(get(&states, "on"), Some(Value::Bool(true)));
        assert_eq!(get(&states, "mode"), Some(Value::Text("froid".into())));
        assert_eq!(get(&states, "target_temperature"), Some(Value::Int(24)));
        assert_eq!(get(&states, "temperature"), Some(Value::Float(29.4)));
        let heat = decode(
            Adapter::Climate,
            payload(&json!({"system_mode": "heat",
            "occupied_cooling_setpoint": 24, "occupied_heating_setpoint": 20})),
        );
        assert_eq!(get(&heat, "target_temperature"), Some(Value::Int(20)));
    }

    #[test]
    fn an_off_unit_keeps_its_last_mode() {
        let states = decode(Adapter::Climate, payload(&json!({"system_mode": "off"})));
        assert_eq!(get(&states, "on"), Some(Value::Bool(false)));
        assert_eq!(get(&states, "mode"), None, "the last mode stays published");
    }

    #[test]
    fn climate_orders_become_z2m_payloads() {
        let on = |mode| encode(Adapter::Climate, "on", &Value::Bool(true), mode);
        assert_eq!(on(Some("chaud")), Some(Ok(json!({"system_mode": "heat"}))));
        assert_eq!(on(None), Some(Ok(json!({"system_mode": "cool"}))));
        assert_eq!(
            encode(Adapter::Climate, "on", &Value::Bool(false), Some("froid")),
            Some(Ok(json!({"system_mode": "off"})))
        );
        assert_eq!(
            encode(
                Adapter::Climate,
                "mode",
                &Value::Text("déshumidification".into()),
                None
            ),
            Some(Ok(json!({"system_mode": "dry"})))
        );
        let target = |mode, t| {
            encode(
                Adapter::Climate,
                "target_temperature",
                &Value::Float(t),
                mode,
            )
        };
        assert_eq!(
            target(Some("froid"), 23.3),
            Some(Ok(json!({"occupied_cooling_setpoint": 23.5})))
        );
        assert_eq!(
            target(Some("chaud"), 21.0),
            Some(Ok(json!({"occupied_heating_setpoint": 21.0})))
        );
        assert!(
            encode(Adapter::Climate, "mode", &Value::Text("x".into()), None)
                .unwrap()
                .is_err()
        );
        assert_eq!(encode(Adapter::None, "on", &Value::Bool(true), None), None);
        assert_eq!(
            encode(
                Adapter::Climate,
                "system_mode",
                &Value::Text("cool".into()),
                None
            ),
            None
        );
    }

    #[test]
    fn devices_are_recognised_by_their_points() {
        let state = point(
            "state",
            String::new(),
            Kind::Enum {
                values: ["OPEN", "CLOSE", "STOP"].map(Arc::from).to_vec(),
            },
            true,
            None,
            Semantic::Other,
        );
        let position = point(
            "position",
            String::new(),
            Kind::Binary,
            true,
            None,
            Semantic::Other,
        );
        assert_eq!(detect(&[state, position]), Adapter::Cover);
        let sm = point(
            "system_mode",
            String::new(),
            Kind::Binary,
            true,
            None,
            Semantic::Other,
        );
        let lt = point(
            "local_temperature",
            String::new(),
            Kind::Binary,
            false,
            None,
            Semantic::Other,
        );
        assert_eq!(detect(&[sm, lt]), Adapter::Climate);
        assert_eq!(detect(&[]), Adapter::None);
    }
}
