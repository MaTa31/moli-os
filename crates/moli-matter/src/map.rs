//! Matter attributes → Moli points, following the conventions of the other
//! light drivers: `brightness` in percent, `color` as « #rrggbb », `color_temp`
//! in mireds with the lamp's real range. One table for the plain attributes;
//! the colour (two attributes, x and y) is the one special case.

use moli_core::color::{srgb_to_xy, xy_to_srgb};
use moli_core::{Access, Device, DeviceId, InstanceId, Kind, PointSpec, Semantic, Unit, Value};
use moli_i18n::tr;
use serde_json::{Value as Json, json};

type Read = fn(&Json) -> Option<Value>;

struct Row {
    cluster: u32,
    attribute: u32,
    key: &'static str,
    label: &'static str,
    /// `None`: a boolean point.
    range: Option<(f64, f64)>,
    unit: Option<Unit>,
    semantic: Semantic,
    read: Read,
}

fn boolean(v: &Json) -> Option<Value> {
    v.as_bool().map(Value::Bool)
}
fn int(v: &Json) -> Option<Value> {
    v.as_i64().map(Value::Int)
}
/// Level 0..=254 → percent, one decimal.
fn percent(v: &Json) -> Option<Value> {
    v.as_f64()
        .map(|x| Value::Float((x / 254.0 * 1000.0).round() / 10.0))
}
/// Hundredths (temperature in °C, humidity in %).
fn hundredths(v: &Json) -> Option<Value> {
    v.as_f64().map(|x| Value::Float(x / 100.0))
}
/// Measured value = 10000·log10(lux) + 1; 0 means "below the range".
fn lux(v: &Json) -> Option<Value> {
    let x = v.as_f64()?;
    Some(Value::Float(if x < 1.0 {
        0.0
    } else {
        ((10f64.powf((x - 1.0) / 10000.0)) * 10.0).round() / 10.0
    }))
}
/// Occupancy is a bitmap; bit 0 is "occupied".
fn occupied(v: &Json) -> Option<Value> {
    v.as_u64().map(|x| Value::Bool(x & 1 == 1))
}

#[allow(clippy::too_many_arguments)]
const fn row(
    cluster: u32,
    attribute: u32,
    key: &'static str,
    label: &'static str,
    range: Option<(f64, f64)>,
    unit: Option<Unit>,
    semantic: Semantic,
    read: Read,
) -> Row {
    Row {
        cluster,
        attribute,
        key,
        label,
        range,
        unit,
        semantic,
        read,
    }
}

static ROWS: [Row; 8] = [
    row(6, 0, "state", "etat", None, None, Semantic::OnOff, boolean),
    row(
        8,
        0,
        "brightness",
        "luminosite",
        Some((0.0, 100.0)),
        Some(Unit::Percent),
        Semantic::Brightness,
        percent,
    ),
    row(
        768,
        7,
        "color_temp",
        "temperature_couleur",
        None,
        None,
        Semantic::ColorTemp,
        int,
    ),
    row(
        1026,
        0,
        "temperature",
        "temperature",
        Some((-50.0, 100.0)),
        Some(Unit::Celsius),
        Semantic::Temperature,
        hundredths,
    ),
    row(
        1029,
        0,
        "humidity",
        "humidite",
        Some((0.0, 100.0)),
        Some(Unit::Percent),
        Semantic::Humidity,
        hundredths,
    ),
    row(
        1024,
        0,
        "illuminance",
        "eclairement",
        Some((0.0, 200_000.0)),
        Some(Unit::Lux),
        Semantic::Illuminance,
        lux,
    ),
    row(
        1030,
        0,
        "occupancy",
        "presence",
        None,
        None,
        Semantic::Occupancy,
        occupied,
    ),
    row(
        69,
        0,
        "contact",
        "contact",
        None,
        None,
        Semantic::Contact,
        boolean,
    ),
];

/// The point key of an attribute on an endpoint: endpoint 1 plain, others
/// prefixed so two endpoints never collide.
fn key(endpoint: u32, key: &str) -> String {
    if endpoint == 1 {
        key.into()
    } else {
        format!("e{endpoint}_{key}")
    }
}

fn label(name: &str) -> String {
    match name {
        "etat" => tr!("pilotes.matter.etat"),
        "luminosite" => tr!("pilotes.matter.luminosite"),
        "temperature_couleur" => tr!("pilotes.matter.temperature_couleur"),
        "couleur" => tr!("pilotes.matter.couleur"),
        "temperature" => tr!("pilotes.matter.temperature"),
        "humidite" => tr!("pilotes.matter.humidite"),
        "eclairement" => tr!("pilotes.matter.eclairement"),
        "presence" => tr!("pilotes.matter.presence"),
        _ => tr!("pilotes.matter.contact"),
    }
}

/// `"1/768/7"` → (1, 768, 7).
fn split(path: &str) -> Option<(u32, u32, u32)> {
    let mut it = path.split('/').map(str::parse::<u32>);
    Some((it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?))
}

/// One attribute update → (point key, value). `None` for what Moli does not
/// model, or a null (an unset attribute). The colour is not here: see [`Color`].
pub fn decode(path: &str, raw: &Json) -> Option<(String, Value)> {
    let (endpoint, cluster, attribute) = split(path)?;
    let row = ROWS
        .iter()
        .find(|r| r.cluster == cluster && r.attribute == attribute)?;
    Some((key(endpoint, row.key), (row.read)(raw)?))
}

/// The colour of a light as Matter keeps it: a mode (0 hue/saturation, 1 xy,
/// 2 colour temperature) and the values of each mode. A mode change leaves
/// the other mode's values stale (a lamp back to white keeps its last red x/y),
/// so Moli publishes **one** reading, as Hue does: in white, `color_temp` plus
/// the `color` of that white; in colour, the `color` and no `color_temp`.
/// Colour lives on endpoint 1 only (a Moli `color` point is one per device).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    present: bool,
    mode: Option<u64>,
    mireds: Option<f64>,
    xy: [Option<f64>; 2],
}

impl Color {
    pub fn from_attributes(attributes: &serde_json::Map<String, Json>) -> Self {
        let get = |path: &str| attributes.get(path);
        Self {
            present: attributes.keys().any(|k| k.starts_with("1/768/")),
            mode: get("1/768/8").and_then(Json::as_u64),
            mireds: get("1/768/7").and_then(Json::as_f64),
            xy: [
                get("1/768/3").and_then(Json::as_f64),
                get("1/768/4").and_then(Json::as_f64),
            ],
        }
    }

    /// Applies an attribute update; `false` when it is not a colour attribute.
    pub fn update(&mut self, path: &str, raw: &Json) -> bool {
        match split(path) {
            Some((1, 768, 3)) => self.xy[0] = raw.as_f64(),
            Some((1, 768, 4)) => self.xy[1] = raw.as_f64(),
            Some((1, 768, 7)) => self.mireds = raw.as_f64(),
            Some((1, 768, 8)) => self.mode = raw.as_u64(),
            _ => return false,
        }
        self.present = true;
        true
    }

    /// The points to publish: `color` and `color_temp` (null in colour mode).
    pub fn states(&self) -> Vec<(&'static str, Value)> {
        if !self.present {
            return Vec::new();
        }
        let mireds = self.mireds.filter(|m| *m > 0.0);
        let white = self.mode == Some(2) || (self.mode.is_none() && mireds.is_some());
        match (white, mireds) {
            (true, Some(m)) => vec![
                ("color", Value::Text(white_hex(m).into())),
                #[allow(clippy::cast_possible_truncation)]
                ("color_temp", Value::Int(m.round() as i64)),
            ],
            (true, None) => Vec::new(),
            (false, _) => {
                let mut states = Vec::new();
                if let Some(hex) = xy_hex(self.xy) {
                    states.push(("color", Value::Text(hex.into())));
                }
                states.push(("color_temp", Value::Null));
                states
            }
        }
    }
}

/// The two raw chromaticity values (0..=65279) → « #rrggbb ».
fn xy_hex(xy: [Option<f64>; 2]) -> Option<String> {
    xy_to_srgb(xy[0]? / 65536.0, xy[1]? / 65536.0)
}

/// « #rrggbb » of a white of `mireds` (blackbody approximation, Tanner Helland).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn white_hex(mireds: f64) -> String {
    let t = (1e6 / mireds).clamp(1000.0, 40000.0) / 100.0;
    let red = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
    };
    let green = if t <= 66.0 {
        99.470_802_586_1 * t.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };
    let byte = |c: f64| c.clamp(0.0, 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(red), byte(green), byte(blue))
}

/// A device, the values it holds, whether it answers.
pub type Decoded = (Device, Vec<(String, Value)>, bool);

fn text(node: &Json, path: &str) -> Option<String> {
    node["attributes"][path]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Whether a point of this meaning can be commanded.
const fn commandable(semantic: Semantic) -> bool {
    matches!(
        semantic,
        Semantic::OnOff | Semantic::Brightness | Semantic::ColorTemp | Semantic::Color
    )
}

/// A node → its device, the values it holds, whether it answers. `color_temp`
/// is the lamp's real range in mireds. `None` when none of its attributes is
/// modelled.
pub fn node(
    instance: &InstanceId,
    node: &Json,
    writable: bool,
    color_temp: [u32; 2],
) -> Option<Decoded> {
    let id = node["node_id"].as_u64()?;
    let attributes = node["attributes"].as_object()?;
    let mut points = Vec::new();
    let mut states = Vec::new();
    let mut paths: Vec<_> = attributes
        .iter()
        .filter_map(|(path, raw)| Some((split(path)?, raw)))
        .collect();
    paths.sort_by_key(|(p, _)| *p);
    for ((endpoint, cluster, attribute), raw) in paths {
        let Some(row) = ROWS
            .iter()
            .find(|r| r.cluster == cluster && r.attribute == attribute)
        else {
            continue;
        };
        let key = key(endpoint, row.key);
        let range = if row.key == "color_temp" {
            Some((f64::from(color_temp[0]), f64::from(color_temp[1])))
        } else {
            row.range
        };
        points.push(PointSpec {
            key: key.as_str().into(),
            label: label(row.label).into(),
            kind: range.map_or(Kind::Binary, |(min, max)| Kind::Numeric {
                min: Some(min),
                max: Some(max),
                step: None,
            }),
            access: Access {
                read: true,
                write: writable && commandable(row.semantic),
            },
            unit: row.unit.clone(),
            semantic: row.semantic,
        });
        if row.key == "color_temp" {
            continue; // published with the colour, see `Color`
        }
        if let Some(value) = (row.read)(raw) {
            states.push((key, value));
        }
    }
    let color = Color::from_attributes(attributes);
    states.extend(color.states().into_iter().map(|(k, v)| (k.to_owned(), v)));
    if attributes.contains_key("1/768/3") && attributes.contains_key("1/768/4") {
        points.push(PointSpec {
            key: "color".into(),
            label: label("couleur").into(),
            kind: Kind::Text,
            access: Access {
                read: true,
                write: writable,
            },
            unit: None,
            semantic: Semantic::Color,
        });
    }
    if points.is_empty() {
        return None;
    }
    let product = text(node, "0/40/3");
    let name = text(node, "0/40/5")
        .unwrap_or_else(|| format!("{} {id}", product.as_deref().unwrap_or("Matter")));
    let device = Device {
        id: DeviceId::new(instance, &id.to_string()),
        instance: instance.clone(),
        native_name: name.as_str().into(),
        manufacturer: text(node, "0/40/1").map(Into::into),
        model: product.map(Into::into),
        description: None,
        native_room: None,
        members: Vec::new(),
        points,
    };
    Some((device, states, node["available"].as_bool().unwrap_or(false)))
}

/// One `device_command`: where, what, with which payload.
#[derive(Debug, PartialEq)]
pub struct Order {
    pub endpoint: u32,
    pub cluster: u32,
    pub name: &'static str,
    pub payload: Json,
}

/// A write on a point → the Matter command that does it. Brightness arrives
/// in percent (what the Moli surfaces send), like the official Govee API
/// (1-100): level = percent × 254 / 100, never below 1 for a lit lamp. Every
/// cast follows a clamp to the target's range.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn encode(key: &str, value: &Value, color_temp: [u32; 2]) -> Result<Order, String> {
    let (endpoint, plain) = match key.strip_prefix('e').and_then(|k| k.split_once('_')) {
        Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            (n.parse().map_err(|_| "endpoint")?, rest)
        }
        _ => (1, key),
    };
    let number = || match value {
        #[allow(clippy::cast_precision_loss)]
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f),
        _ => Err(format!("{key}: a number is expected")),
    };
    let timing = |mut p: Json| {
        p["transitionTime"] = 0.into();
        p["optionsMask"] = 0.into();
        p["optionsOverride"] = 0.into();
        p
    };
    let (cluster, name, payload) = match plain {
        "state" => match value {
            Value::Bool(true) => (6, "On", json!({})),
            Value::Bool(false) => (6, "Off", json!({})),
            _ => return Err("state: a boolean is expected".into()),
        },
        "brightness" => {
            let pct = number()?.clamp(0.0, 100.0);
            let level = (pct * 2.54).round() as u32;
            let level = if pct > 0.0 { level.max(1) } else { 0 };
            (8, "MoveToLevelWithOnOff", timing(json!({ "level": level })))
        }
        "color_temp" => {
            let mireds = number()?
                .round()
                .clamp(f64::from(color_temp[0]), f64::from(color_temp[1]))
                as u32;
            (
                768,
                "MoveToColorTemperature",
                timing(json!({ "colorTemperatureMireds": mireds })),
            )
        }
        "color" => {
            let Value::Text(hex) = value else {
                return Err("color: #rrggbb is expected".into());
            };
            let (x, y) = srgb_to_xy(hex).ok_or("color: #rrggbb is expected")?;
            let raw = |c: f64| (c * 65536.0).round().min(65279.0) as u32;
            (
                768,
                "MoveToColor",
                timing(json!({ "colorX": raw(x), "colorY": raw(y) })),
            )
        }
        _ => return Err(format!("{key}: not commandable")),
    };
    Ok(Order {
        endpoint,
        cluster,
        name,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CT: [u32; 2] = [154, 370];

    fn nodes() -> Vec<Json> {
        serde_json::from_slice(include_bytes!(
            "../../../integrations/matter/fixtures/nodes.json"
        ))
        .unwrap()
    }

    fn instance() -> InstanceId {
        InstanceId::from("matter")
    }

    #[test]
    fn real_nodes_are_mapped() {
        let nodes = nodes();
        assert_eq!(nodes.len(), 9);
        for n in &nodes {
            let (device, states, _) = node(&instance(), n, false, CT).expect("a light");
            let keys: Vec<_> = device.points.iter().map(|p| &*p.key).collect();
            for k in ["state", "brightness", "color_temp", "color"] {
                assert!(keys.contains(&k), "{k} in {keys:?}");
            }
            assert!(device.points.iter().all(|p| !p.access.write), "read-only");
            assert!(states.iter().any(|(k, _)| k == "state"));
        }
    }

    #[test]
    fn a_light_has_its_values() {
        let nodes = nodes();
        let (device, states, available) = node(&instance(), &nodes[1], true, CT).unwrap();
        assert_eq!(&*device.id.to_string(), "matter:4");
        assert_eq!(device.model.as_deref(), Some("H60C1"));
        assert_eq!(&*device.native_name, "H60C1 4");
        assert!(available);
        let get = |k: &str| {
            states
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("state"), Some(Value::Bool(true)));
        assert_eq!(get("brightness"), Some(Value::Float(40.2)), "102/254");
        assert_eq!(get("color_temp"), Some(Value::Int(370)));
        assert!(matches!(get("color"), Some(Value::Text(t)) if t.starts_with('#')));
        let brightness = device.point("brightness").unwrap();
        assert_eq!(brightness.unit, Some(Unit::Percent));
    }

    #[test]
    fn an_unreachable_node_is_flagged() {
        let nodes = nodes();
        let (_, _, available) = node(&instance(), &nodes[3], true, CT).unwrap();
        assert!(!available);
    }

    #[test]
    fn only_writable_nodes_have_commandable_lights() {
        let nodes = nodes();
        let on = |w| node(&instance(), &nodes[1], w, CT).unwrap().0;
        assert!(on(true).points.iter().all(|p| p.access.write));
        assert!(on(false).points.iter().all(|p| !p.access.write));
        let range = on(true).point("color_temp").unwrap().kind.clone();
        assert_eq!(
            range,
            Kind::Numeric {
                min: Some(154.0),
                max: Some(370.0),
                step: None
            }
        );
    }

    #[test]
    fn updates_are_decoded() {
        let some = |k: &str, v| Some((k.to_owned(), v));
        assert_eq!(
            decode("1/6/0", &json!(false)),
            some("state", Value::Bool(false))
        );
        assert_eq!(
            decode("1/8/0", &json!(254)),
            some("brightness", Value::Float(100.0))
        );
        assert_eq!(
            decode("2/1026/0", &json!(2150)),
            some("e2_temperature", Value::Float(21.5))
        );
        assert_eq!(
            decode("1/1030/0", &json!(1)),
            some("occupancy", Value::Bool(true))
        );
        assert_eq!(decode("1/768/7", &Json::Null), None);
        assert_eq!(
            decode("1/768/3", &json!(1)),
            None,
            "colour has its own path"
        );
        assert_eq!(decode("0/40/3", &json!("x")), None);
        assert_eq!(decode("bad", &json!(1)), None);
    }

    #[test]
    fn writes_become_matter_commands() {
        let order = |k, v: Value| encode(k, &v, CT).unwrap();
        assert_eq!(order("state", Value::Bool(true)).name, "On");
        assert_eq!(order("state", Value::Bool(false)).name, "Off");
        assert_eq!(
            order("brightness", Value::Float(100.0)).payload["level"],
            254
        );
        assert_eq!(order("brightness", Value::Int(50)).payload["level"], 127);
        assert_eq!(
            order("brightness", Value::Float(0.1)).payload["level"],
            1,
            "lit stays lit"
        );
        assert_eq!(order("brightness", Value::Int(0)).payload["level"], 0);
        assert_eq!(
            order("color_temp", Value::Int(500)).payload["colorTemperatureMireds"],
            370
        );
        assert_eq!(
            order("color_temp", Value::Int(100)).payload["colorTemperatureMireds"],
            154
        );
        assert_eq!(order("e2_state", Value::Bool(true)).endpoint, 2);
        let red = order("color", Value::Text("#ff0000".into()));
        assert_eq!((red.cluster, red.name), (768, "MoveToColor"));
        assert!(red.payload["colorX"].as_u64().unwrap() > 40_000);
        // Standard sRGB red (x 0.64, y 0.33), not Hue's wide-gamut red (0.70): a lamp that is not a Hue
        // reads these as standard coordinates, and a red outside its gamut came out green on a Govee ribbon.
        assert_eq!(red.payload["colorX"], 41_943);
        assert_eq!(red.payload["colorY"], 21_627);
        assert!(encode("color", &Value::Text("red".into()), CT).is_err());
        assert!(encode("linkquality", &Value::Int(1), CT).is_err());
    }

    fn color(path_values: &[(&str, Json)]) -> Color {
        let mut c = Color::default();
        for (path, v) in path_values {
            assert!(c.update(path, v));
        }
        c
    }

    fn get(states: &[(&'static str, Value)], key: &str) -> Option<Value> {
        states
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
    }

    #[test]
    fn a_white_lamp_shows_its_white_not_its_last_colour() {
        // Last colour was red (x, y stay), then back to white: mode 2, 370 mireds.
        let c = color(&[
            ("1/768/3", json!(45000)),
            ("1/768/4", json!(20000)),
            ("1/768/7", json!(370)),
            ("1/768/8", json!(2)),
        ]);
        let states = c.states();
        assert_eq!(get(&states, "color_temp"), Some(Value::Int(370)));
        let Some(Value::Text(hex)) = get(&states, "color") else {
            panic!("a colour")
        };
        assert_eq!(&*hex, "#ffa758", "2700 K is a warm orange-white, not red");
    }

    #[test]
    fn a_coloured_lamp_has_no_temperature() {
        let c = color(&[
            ("1/768/3", json!(45000)),
            ("1/768/4", json!(20000)),
            ("1/768/7", json!(370)),
            ("1/768/8", json!(1)),
        ]);
        let states = c.states();
        assert_eq!(get(&states, "color_temp"), Some(Value::Null));
        assert!(matches!(get(&states, "color"), Some(Value::Text(t)) if t.starts_with('#')));
    }

    #[test]
    fn colour_waits_for_its_values_and_ignores_other_attributes() {
        let mut c = Color::default();
        assert!(c.states().is_empty(), "a lamp without colour cluster");
        assert!(!c.update("1/6/0", &json!(true)));
        assert!(!c.update("2/768/7", &json!(300)), "endpoint 1 only");
        assert!(c.update("1/768/3", &json!(1)));
        assert_eq!(get(&c.states(), "color"), None, "y is missing");
    }

    #[test]
    fn whites_run_from_orange_to_blue() {
        assert_eq!(white_hex(500.0), "#ff890e", "2000 K");
        assert!(white_hex(154.0).starts_with("#ff"), "6500 K is near white");
        assert!(white_hex(111.0).ends_with("ff"), "9000 K is blue-white");
    }

    #[test]
    fn lux_follows_the_matter_formula() {
        assert_eq!(lux(&json!(1)), Some(Value::Float(1.0)));
        assert_eq!(lux(&json!(30_001)), Some(Value::Float(1000.0)));
        assert_eq!(lux(&json!(0)), Some(Value::Float(0.0)));
    }
}
