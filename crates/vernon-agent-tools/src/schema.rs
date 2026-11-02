use serde_json::{json, Value};

use crate::ToolSpec;

fn spec(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters: json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        }),
    }
}

const NODE: &str = "Node instance name, e.g. 'icp_align1' (see add_node results).";
const PORT: &str = "Port index (integer) or port name (string).";

/// Every tool, in the order models see them.
pub fn all() -> Vec<ToolSpec> {
    vec![
        spec(
            "list_nodes",
            "List node types you can add, optionally for one pipeline stage. Returns id, label, stage and one-line help.",
            json!({"stage": {"type": "string", "description": "geo, capture, clean, surface, print, cfd, fea, solar, flatten or perf."}}),
            &[],
        ),
        spec(
            "describe_node",
            "Full description of a node type: input and output ports with kinds, every parameter with type, range, unit, options and default.",
            json!({"type_id": {"type": "string", "description": "Node type id such as 'clean.icp_align'."}}),
            &["type_id"],
        ),
        spec(
            "get_graph",
            "The current graph: every node with its type, parameter values, input wiring and position.",
            json!({}),
            &[],
        ),
        spec(
            "add_node",
            "Add a node of a type with default parameters; optional initial params. Returns its instance name and ports.",
            json!({
                "type_id": {"type": "string"},
                "name": {"type": "string", "description": "Optional instance name (letters, digits, _ - . space)."},
                "params": {"type": "object", "description": "Initial parameter values by name."},
                "x": {"type": "number"}, "y": {"type": "number"}
            }),
            &["type_id"],
        ),
        spec(
            "set_param",
            "Set one parameter of a node. Values are validated against the type, range, unit and options.",
            json!({
                "node": {"type": "string", "description": NODE},
                "param": {"type": "string"},
                "value": {"description": "number, integer, boolean, string, or [x,y,z] for vec3."}
            }),
            &["node", "param", "value"],
        ),
        spec(
            "connect",
            "Wire an output of one node into an input of another. Kinds must fit and the wire must not make a cycle. Omit to_input to use the first free input that fits.",
            json!({
                "from": {"type": "string", "description": NODE},
                "to": {"type": "string", "description": NODE},
                "from_output": {"description": PORT},
                "to_input": {"description": PORT}
            }),
            &["from", "to"],
        ),
        spec(
            "disconnect",
            "Remove the wire into an input.",
            json!({"node": {"type": "string"}, "input": {"description": PORT}}),
            &["node", "input"],
        ),
        spec(
            "delete_node",
            "Delete a node; wires from it are cut.",
            json!({"node": {"type": "string"}}),
            &["node"],
        ),
        spec(
            "move_node",
            "Set the position of a node in the editor.",
            json!({"node": {"type": "string"}, "x": {"type": "number"}, "y": {"type": "number"}}),
            &["node", "x", "y"],
        ),
        spec(
            "rename_node",
            "Rename a node. The new name must be unique.",
            json!({"node": {"type": "string"}, "new_name": {"type": "string"}}),
            &["node", "new_name"],
        ),
        spec(
            "cook",
            "Cook one node (and what it needs), or every node when 'node' is omitted. Returns per-node status, reports (metrics, warnings) and failed gates. Nodes that drive external programs only run when their 'run' switch was set.",
            json!({"node": {"type": "string", "description": NODE}}),
            &[],
        ),
        spec(
            "read_report",
            "Reports produced by a node (cooked if needed): metrics with units and limits, and notes.",
            json!({"node": {"type": "string"}}),
            &["node"],
        ),
        spec(
            "get_metric",
            "One metric of a node's report: value, unit, limit and whether it passes.",
            json!({"node": {"type": "string"}, "metric": {"type": "string"}}),
            &["node", "metric"],
        ),
        spec(
            "list_gates",
            "Every limited metric across the graph (or one node) with value, limit, margin and pass/fail. Failures listed under a node's expect_fail are marked expected.",
            json!({"node": {"type": "string"}}),
            &[],
        ),
        spec(
            "measure_geometry",
            "Bounds (mm), size, point and face counts, surface area (mm2) and watertightness of a node's geometry output.",
            json!({"node": {"type": "string"}, "output": {"description": PORT}}),
            &["node"],
        ),
        spec("undo", "Undo the last change (one tool call, or one begin_group..end_group group).", json!({}), &[]),
        spec("redo", "Redo the last undone change.", json!({}), &[]),
        spec(
            "begin_group",
            "Start a group: every change until end_group is one undo step.",
            json!({}),
            &[],
        ),
        spec("end_group", "End the group started by begin_group.", json!({}), &[]),
        spec(
            "load_graph",
            "Load a .vrn graph from the work folder (relative path, no '..'). Replaces the current graph; undoable.",
            json!({"path": {"type": "string", "description": "e.g. 'bodykit.vrn'"}}),
            &["path"],
        ),
        spec(
            "save_graph",
            "Save the graph as a .vrn file in the work folder (relative path, no '..').",
            json!({"path": {"type": "string"}}),
            &["path"],
        ),
    ]
}
