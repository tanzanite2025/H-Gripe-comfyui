//! Studio workflow graph schema (the JSON the renderer serializes) plus the
//! small value-coercion helpers shared by the execution engine and the
//! PSD-export node. These types are deliberately renderer-agnostic and mirror
//! the TypeScript `WorkflowGraph` model.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(crate) struct StudioWorkflowGraph {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) nodes: Vec<StudioGraphNode>,
    #[serde(default)]
    pub(crate) edges: Vec<StudioGraphEdge>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StudioGraphNode {
    pub(crate) id: String,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) params: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StudioGraphEdge {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) source_port: String,
    pub(crate) target: String,
    pub(crate) target_port: String,
}

pub(crate) fn studio_output_map<const N: usize>(
    entries: [(&str, Value); N],
) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

pub(crate) fn studio_value_to_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

pub(crate) fn studio_value_to_number(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(value)) => value.parse::<f64>().unwrap_or(0.0),
        Some(Value::Bool(value)) => {
            if *value {
                1.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

pub(crate) fn studio_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(false),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub(crate) fn studio_non_empty(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}
