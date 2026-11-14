// DOM walker for the Frost inspector (Round 5).
//
// Runs inside the inspected page via `WebView::evaluate_javascript` and
// returns a JSON string: `{url, capturedAt, truncated, nodeCount, root}`.
// The tree shape matches `frost_inspector::DomNode` field-for-field
// (`node_id`, `tag`, `attributes`, `text`, `children`, `computed_style`,
// `box_model`), so the Rust side deserializes it directly.
//
// Limits (the walker enforces them, so a huge page cannot produce a
// multi-megabyte string): at most MAX_NODES nodes, MAX_DEPTH deep;
// element text truncated to MAX_TEXT chars, attribute values to
// MAX_ATTR. `truncated: true` in the output says a limit was hit.
//
// Deliberately skipped in v1: comments, doctype, iframe sub-documents
// (an <iframe> shows with no children), shadow DOM. The walker never
// throws: every per-node read is guarded, so a weird node yields an
// empty style/box instead of killing the whole snapshot.
(function () {
  var MAX_NODES = 8000;
  var MAX_DEPTH = 64;
  var MAX_TEXT = 200;
  var MAX_ATTR = 256;
  var count = 1; // node 0 is the #document root, made below
  var truncated = false;

  function trunc(s, n) {
    s = String(s);
    return s.length > n ? s.slice(0, n) + "\u2026" : s;
  }

  function px(v) {
    var n = parseFloat(v);
    return isNaN(n) ? 0 : n;
  }

  function styleOf(el) {
    try {
      var cs = getComputedStyle(el);
      return {
        color: cs.color || null,
        background_color: cs.backgroundColor || null,
        font_family: cs.fontFamily || null,
        font_size: cs.fontSize || null,
        font_weight: cs.fontWeight || null,
        display: cs.display || null,
        position: cs.position || null,
        width: cs.width || null,
        height: cs.height || null,
      };
    } catch (e) {
      return {};
    }
  }

  function boxOf(el) {
    try {
      var r = el.getBoundingClientRect();
      var cs = getComputedStyle(el);
      return {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
        margin: [px(cs.marginTop), px(cs.marginRight), px(cs.marginBottom), px(cs.marginLeft)],
        padding: [px(cs.paddingTop), px(cs.paddingRight), px(cs.paddingBottom), px(cs.paddingLeft)],
        border: [px(cs.borderTopWidth), px(cs.borderRightWidth), px(cs.borderBottomWidth), px(cs.borderLeftWidth)],
      };
    } catch (e) {
      return null;
    }
  }

  function walk(node, depth) {
    if (count >= MAX_NODES || depth > MAX_DEPTH) {
      truncated = true;
      return null;
    }
    // Text nodes: whitespace-only text carries no information and would
    // drown the tree (pretty-printed HTML is full of it).
    if (node.nodeType === 3) {
      var t = node.nodeValue || "";
      if (!t.trim()) return null;
      return {
        node_id: count++,
        tag: "#text",
        attributes: {},
        text: trunc(t.trim(), MAX_TEXT),
        children: [],
        computed_style: {},
        box_model: null,
      };
    }
    if (node.nodeType !== 1) return null; // elements only past this point
    var out = {
      node_id: count++,
      tag: (node.tagName || "unknown").toLowerCase(),
      attributes: {},
      text: null,
      children: [],
      computed_style: {},
      box_model: null,
    };
    for (var i = 0; i < node.attributes.length; i++) {
      var a = node.attributes[i];
      out.attributes[a.name] = trunc(a.value, MAX_ATTR);
    }
    // The element's own direct text (for the details pane).
    var direct = "";
    for (var c = 0; c < node.childNodes.length; c++) {
      var ch = node.childNodes[c];
      if (ch.nodeType === 3) direct += ch.nodeValue;
    }
    if (direct.trim()) out.text = trunc(direct.trim(), MAX_TEXT);
    out.computed_style = styleOf(node);
    out.box_model = boxOf(node);
    for (var j = 0; j < node.childNodes.length; j++) {
      var child = walk(node.childNodes[j], depth + 1);
      if (child) out.children.push(child);
    }
    return out;
  }

  var html = walk(document.documentElement, 0);
  var root = {
    node_id: 0,
    tag: "#document",
    attributes: {},
    text: null,
    children: html ? [html] : [],
    computed_style: {},
    box_model: null,
  };
  return JSON.stringify({
    url: location.href,
    capturedAt: Date.now(),
    truncated: truncated,
    nodeCount: count,
    root: root,
  });
})();
