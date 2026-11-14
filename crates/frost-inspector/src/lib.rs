//! frost-inspector: the Frost HTML inspector (built in-house, because
//! Servo ships no inspector of its own).
//!
//! Round 1 is scaffolding: the snapshot types ([`DomNode`],
//! [`ComputedStyle`], [`BoxModel`]), the [`Inspector`] trait, and a
//! [`StubInspector`] that returns an empty-document snapshot. The real
//! implementation (Round 4+) will pull the DOM out of Servo — Servo's
//! constellation owns the live DOM, so the inspector needs a fork-side
//! hook that serializes a subtree on request (see `FORK.md`, patch F-002).
//! The UI contract is fixed now so the inspector page (`frost://inspect`)
//! can be built against the trait without waiting for the engine hook.
//!
//! Design decisions (locked in now):
//!
//! * A snapshot is a plain owned tree — no live handles into Servo. The
//!   inspector re-snapshots on demand (on selection change, on DOM mutation
//!   events in a later round). Owned trees are easy to serialize to the
//!   inspector page and easy to test.
//! * Node ids are stable within one snapshot only. The picker maps a
//!   viewport point to the deepest node id at that point; the UI then looks
//!   the node up in the snapshot.
//! * Computed style is a small fixed struct, not a full CSSOM — the
//!   inspector shows the handful of properties people actually read first
//!   (color, background, font, display, box). More properties come later
//!   without breaking the struct (all fields are `Option`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A node in a DOM snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomNode {
    /// Stable within this snapshot only.
    pub node_id: u64,
    /// Tag name (`div`, `span`, …) or `#text` / `#document` / `#comment`.
    pub tag: String,
    pub attributes: HashMap<String, String>,
    /// Text content for `#text` nodes (and short text for elements).
    pub text: Option<String>,
    pub children: Vec<DomNode>,
    pub computed_style: ComputedStyle,
    pub box_model: Option<BoxModel>,
}

impl DomNode {
    pub fn new(node_id: u64, tag: impl Into<String>) -> Self {
        Self {
            node_id,
            tag: tag.into(),
            attributes: HashMap::new(),
            text: None,
            children: Vec::new(),
            computed_style: ComputedStyle::default(),
            box_model: None,
        }
    }

    /// Depth-first search for a node id.
    pub fn find(&self, node_id: u64) -> Option<&DomNode> {
        if self.node_id == node_id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(node_id))
    }

    /// Total nodes in this subtree, including self.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(DomNode::count).sum::<usize>()
    }

    /// The deepest node whose box contains `(x, y)`, or self if none of the
    /// children contain the point. Used by the element picker.
    pub fn deepest_at(&self, x: f32, y: f32) -> &DomNode {
        for child in &self.children {
            if let Some(b) = &child.box_model {
                if x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height {
                    return child.deepest_at(x, y);
                }
            }
        }
        self
    }
}

/// The computed-style properties the inspector shows. Everything is
/// `Option` — the engine fills in what Servo's style system reports and
/// leaves the rest blank rather than inventing values.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComputedStyle {
    pub color: Option<String>,
    pub background_color: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<String>,
    pub font_weight: Option<String>,
    pub display: Option<String>,
    pub position: Option<String>,
    pub width: Option<String>,
    pub height: Option<String>,
}

/// Layout box for a node, in CSS pixels relative to the viewport origin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxModel {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// [top, right, bottom, left]
    pub margin: [f32; 4],
    pub padding: [f32; 4],
    pub border: [f32; 4],
}

impl BoxModel {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
            margin: [0.0; 4],
            padding: [0.0; 4],
            border: [0.0; 4],
        }
    }
}

/// A whole-page snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomSnapshot {
    pub url: String,
    pub root: DomNode,
    pub node_count: usize,
    /// Unix milliseconds when the walker ran (Round 5: the inspector
    /// page renders it as a local time; 0 means "unknown", e.g. the
    /// Round 1 `empty()` fixture).
    #[serde(default)]
    pub captured_at_unix_ms: u64,
    /// True when the walker hit its node/depth limit and the tree is
    /// incomplete (Round 5: the inspector page says so).
    #[serde(default)]
    pub truncated: bool,
}

impl DomSnapshot {
    /// An empty document: just `#document` > `html` > `head` + `body`.
    pub fn empty(url: impl Into<String>) -> Self {
        let mut root = DomNode::new(0, "#document");
        let mut html = DomNode::new(1, "html");
        html.children.push(DomNode::new(2, "head"));
        html.children.push(DomNode::new(3, "body"));
        root.children.push(html);
        let node_count = root.count();
        Self {
            url: url.into(),
            root,
            node_count,
            captured_at_unix_ms: 0,
            truncated: false,
        }
    }

    pub fn find(&self, node_id: u64) -> Option<&DomNode> {
        self.root.find(node_id)
    }
}

/// The DOM walker script (Round 5): runs inside the inspected page via
/// `WebView::evaluate_javascript` and returns a JSON string — see
/// `walk.js`. The tree shape matches [`DomNode`] field-for-field, so
/// [`snapshot_from_json`] deserializes it directly.
pub const DOM_WALK_SCRIPT: &str = include_str!("walk.js");

/// The walker output envelope (keys as the script emits them).
#[derive(Debug, Deserialize)]
struct WalkerOutput {
    url: String,
    #[serde(rename = "capturedAt")]
    captured_at: u64,
    #[serde(default)]
    truncated: bool,
    #[serde(rename = "nodeCount")]
    node_count: usize,
    root: DomNode,
}

/// Parse the JSON string the walker script returned into a snapshot.
/// Rejects anything that is not a `#document`-rooted walker envelope,
/// so a page that redefined something (or a script error that returned
/// a stray string) never becomes a fake tree.
pub fn snapshot_from_json(json: &str) -> Result<DomSnapshot, String> {
    let out: WalkerOutput =
        serde_json::from_str(json).map_err(|e| format!("inspector JSON did not parse: {e}"))?;
    if out.root.tag != "#document" {
        return Err(format!(
            "inspector JSON has root tag {:?}, expected \"#document\"",
            out.root.tag
        ));
    }
    Ok(DomSnapshot {
        url: out.url,
        root: out.root,
        node_count: out.node_count,
        captured_at_unix_ms: out.captured_at,
        truncated: out.truncated,
    })
}

/// Picker state: what the user is hovering / has selected with the
/// element-picker tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PickerState {
    pub active: bool,
    pub hovered_node: Option<u64>,
    pub selected_node: Option<u64>,
}

/// Errors from the inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectorError {
    /// The engine has no live DOM for this URL (yet) — e.g. the page has
    /// not finished loading, or the fork-side snapshot hook is not built.
    NoDom(String),
    NodeNotFound(u64),
}

impl std::fmt::Display for InspectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InspectorError::NoDom(msg) => write!(f, "no DOM available: {msg}"),
            InspectorError::NodeNotFound(id) => write!(f, "no node with id {id}"),
        }
    }
}

impl std::error::Error for InspectorError {}

/// The inspector: snapshot a page's DOM, pick elements by viewport point.
pub trait Inspector {
    /// Snapshot the live DOM of the page at `url`.
    fn inspect(&self, url: &str) -> Result<DomSnapshot, InspectorError>;
    /// Feed a viewport point to the picker; returns the picked node id, if any.
    fn pick_element(&mut self, snapshot: &DomSnapshot, x: f32, y: f32) -> Option<u64>;
    /// Currently selected node id, if the picker has one.
    fn selected(&self) -> Option<u64>;
    /// Current picker state (for the UI to render the highlight overlay).
    fn picker_state(&self) -> &PickerState;
}

/// Round 1 stub: returns an empty document for any URL and picks the root
/// node. It exists so the inspector page and its tests can be built now;
/// the real inspector replaces this struct, not its trait.
#[derive(Debug, Default)]
pub struct StubInspector {
    picker: PickerState,
}

impl StubInspector {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Inspector for StubInspector {
    fn inspect(&self, url: &str) -> Result<DomSnapshot, InspectorError> {
        Ok(DomSnapshot::empty(url))
    }

    fn pick_element(&mut self, snapshot: &DomSnapshot, x: f32, y: f32) -> Option<u64> {
        let node = snapshot.root.deepest_at(x, y);
        self.picker.active = true;
        self.picker.hovered_node = Some(node.node_id);
        self.picker.selected_node = Some(node.node_id);
        Some(node.node_id)
    }

    fn selected(&self) -> Option<u64> {
        self.picker.selected_node
    }

    fn picker_state(&self) -> &PickerState {
        &self.picker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tree() -> DomNode {
        let mut root = DomNode::new(0, "#document");
        let mut html = DomNode::new(1, "html");
        // Every rendered element has a box in real snapshots; the fixture
        // must too, or the picker cannot descend past it.
        html.box_model = Some(BoxModel::new(0.0, 0.0, 800.0, 600.0));
        let mut body = DomNode::new(2, "body");
        body.box_model = Some(BoxModel::new(0.0, 0.0, 800.0, 600.0));
        let mut div = DomNode::new(3, "div");
        div.attributes.insert("id".into(), "main".into());
        div.box_model = Some(BoxModel::new(10.0, 20.0, 100.0, 50.0));
        let mut text = DomNode::new(4, "#text");
        text.text = Some("hello".into());
        div.children.push(text);
        body.children.push(div);
        html.children.push(body);
        root.children.push(html);
        root
    }

    #[test]
    fn node_find_and_count_work() {
        let root = sample_tree();
        assert_eq!(root.count(), 5);
        let div = root.find(3).unwrap();
        assert_eq!(div.tag, "div");
        assert_eq!(div.attributes.get("id").map(String::as_str), Some("main"));
        assert!(root.find(99).is_none());
    }

    #[test]
    fn deepest_at_picks_the_innermost_box() {
        let root = sample_tree();
        // Inside the div's box.
        assert_eq!(root.deepest_at(50.0, 40.0).node_id, 3);
        // Inside body but outside the div.
        assert_eq!(root.deepest_at(500.0, 500.0).node_id, 2);
    }

    #[test]
    fn empty_snapshot_has_document_html_head_body() {
        let snap = DomSnapshot::empty("https://example.org");
        assert_eq!(snap.node_count, 4);
        assert_eq!(snap.root.tag, "#document");
        assert!(snap.find(3).is_some());
    }

    #[test]
    fn stub_inspector_picks_and_reports_selection() {
        let mut insp = StubInspector::new();
        let snap = insp.inspect("https://example.org").unwrap();
        let picked = insp.pick_element(&snap, 5.0, 5.0);
        assert_eq!(picked, insp.selected());
        assert!(insp.picker_state().active);
    }

    #[test]
    fn snapshot_serializes_to_json() {
        let snap = DomSnapshot::empty("https://example.org");
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("example.org"));
        let back: DomSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.node_count, 4);
    }

    /// The walker output for a page shaped like example.org:
    /// html > head + body > div > h1 + p. The walker emits
    /// `computed_style`/`box_model` keys matching the Rust field
    /// names, so the root deserializes straight into `DomNode`.
    /// A raw string, not `json!`: the nesting is deep enough to hit
    /// the macro's recursion limit.
    fn example_org_walker_json() -> String {
        r##"{
            "url": "https://example.org/",
            "capturedAt": 1794643768000,
            "truncated": false,
            "nodeCount": 8,
            "root": {
                "node_id": 0, "tag": "#document", "attributes": {},
                "text": null, "children": [{
                    "node_id": 1, "tag": "html", "attributes": {},
                    "text": null, "children": [
                        {"node_id": 2, "tag": "head", "attributes": {},
                         "text": null, "children": [],
                         "computed_style": {}, "box_model": null},
                        {"node_id": 3, "tag": "body", "attributes": {},
                         "text": null, "children": [{
                            "node_id": 4, "tag": "div", "attributes": {},
                            "text": null, "children": [
                                {"node_id": 5, "tag": "h1", "attributes": {},
                                 "text": "Example Domain", "children": [
                                    {"node_id": 6, "tag": "#text", "attributes": {},
                                     "text": "Example Domain", "children": [],
                                     "computed_style": {}, "box_model": null}],
                                 "computed_style": {"display": "block", "font_size": "32px"},
                                 "box_model": {"x": 20.0, "y": 20.0, "width": 1240.0, "height": 43.0,
                                               "margin": [0.0, 0.0, 0.0, 0.0],
                                               "padding": [0.0, 0.0, 0.0, 0.0],
                                               "border": [0.0, 0.0, 0.0, 0.0]}},
                                {"node_id": 7, "tag": "p", "attributes": {},
                                 "text": "This domain is for use", "children": [],
                                 "computed_style": {}, "box_model": null}],
                            "computed_style": {}, "box_model": null}],
                         "computed_style": {}, "box_model": null}],
                    "computed_style": {}, "box_model": null}],
                "computed_style": {}, "box_model": null
            }
        }"##
        .to_string()
    }

    #[test]
    fn walker_json_parses_into_a_real_tree() {
        let snap = snapshot_from_json(&example_org_walker_json()).unwrap();
        assert_eq!(snap.url, "https://example.org/");
        assert_eq!(snap.captured_at_unix_ms, 1794643768000);
        assert!(!snap.truncated);
        assert_eq!(snap.node_count, 8);
        // html > body > div > h1 + p, with the h1's text node.
        let h1 = snap.find(5).unwrap();
        assert_eq!(h1.tag, "h1");
        assert_eq!(h1.text.as_deref(), Some("Example Domain"));
        assert_eq!(
            h1.computed_style.font_size.as_deref(),
            Some("32px")
        );
        let rect = h1.box_model.as_ref().unwrap();
        assert_eq!((rect.x, rect.y, rect.width), (20.0, 20.0, 1240.0));
        let text = snap.find(6).unwrap();
        assert_eq!(text.tag, "#text");
        assert_eq!(snap.find(7).unwrap().tag, "p");
        assert_eq!(snap.root.count(), 8);
    }

    #[test]
    fn walker_json_rejects_garbage_and_wrong_roots() {
        assert!(snapshot_from_json("not json").is_err());
        assert!(snapshot_from_json("{\"url\":\"x\"}").is_err());
        let mut wrong = example_org_walker_json();
        wrong = wrong.replacen("\"#document\"", "\"html\"", 1);
        assert!(snapshot_from_json(&wrong).is_err());
    }

    #[test]
    fn walk_script_has_the_walker_contract() {
        // Cheap guard against accidental edits: the script must return
        // a JSON string with the envelope keys and enforce its limits.
        assert!(DOM_WALK_SCRIPT.contains("JSON.stringify"));
        assert!(DOM_WALK_SCRIPT.contains("capturedAt"));
        assert!(DOM_WALK_SCRIPT.contains("getBoundingClientRect"));
        assert!(DOM_WALK_SCRIPT.contains("getComputedStyle"));
        assert!(DOM_WALK_SCRIPT.contains("MAX_NODES"));
        assert!(DOM_WALK_SCRIPT.contains("node_id"));
    }
}
