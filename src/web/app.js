// Draws the graph from /api/graph with vis-network and fills the sidebar.

const PALETTE = ["#4a90d9", "#e67e22", "#27ae60", "#8e44ad", "#c0392b", "#16a085", "#d4a017", "#7f8c8d", "#e84393", "#2c3e50"];
// Fields tried, in order, for a node's display name.
const NAME_FIELDS = ["name", "title", "label", "username", "email"];
// Must match MAX_PER_TABLE in src/graph/mod.rs.
const MAX_PER_TABLE = 1000;

// Colors used while hovering: everything outside the hovered neighbourhood fades.
const EDGE_COLOR = "#999";
const EDGE_FONT_COLOR = "#777";
const NODE_FONT_COLOR = "#222";
const FADED = "#e2e2e2";
const INVISIBLE = "rgba(0,0,0,0)";

// ---- Helpers ----

const esc = s => String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;
const byId = id => document.getElementById(id);

// Turns a field value into short text: strings as-is, objects like
// { first, last } joined from their text parts.
function asText(v) {
  if (typeof v === "string" || typeof v === "number") return String(v);
  if (v && typeof v === "object" && !Array.isArray(v)) {
    const parts = Object.values(v).filter(x => typeof x === "string" || typeof x === "number");
    if (parts.length) return parts.join(" ");
  }
  return null;
}

function displayName(node) {
  for (const f of NAME_FIELDS) {
    const text = node.data && asText(node.data[f]);
    if (text) return text;
  }
  return node.id;
}

// ---- Data ----

async function fetchGraph() {
  const res = await fetch("/api/graph");
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}

// Indexes the raw data and drops edges whose ends aren't loaded.
function prepare({ nodes, edges, truncated }) {
  const nodeById = new Map(nodes.map(n => [n.id, n]));
  const tables = [...new Set(nodes.map(n => n.table))].sort();
  const color = new Map(tables.map((t, i) => [t, PALETTE[i % PALETTE.length]]));
  const drawable = edges.filter(e => nodeById.has(e.from) && nodeById.has(e.to));
  const edgeById = new Map(drawable.map(e => [e.id, e]));
  const hiddenEdges = edges.length - drawable.length;
  const nameOf = id => nodeById.has(id) ? displayName(nodeById.get(id)) : id;
  return { nodes, edges: drawable, nodeById, edgeById, tables, color, truncated, hiddenEdges, nameOf };
}

// ---- Sidebar ----

function renderSummary(g) {
  byId("summary").textContent =
    `${plural(g.nodes.length, "record")} in ${plural(g.tables.length, "table")}, ${plural(g.edges.length, "relation")}`;

  const warnings = [];
  if (g.truncated.length) warnings.push(`Only the first ${MAX_PER_TABLE} records shown for: ${g.truncated.join(", ")}`);
  if (g.hiddenEdges) warnings.push(`${plural(g.hiddenEdges, "relation")} point at records that aren't shown`);
  byId("warnings").innerHTML = warnings.map(w => `<p class="warn">${esc(w)}</p>`).join("");
}

function renderLegend(g) {
  byId("legend").innerHTML = g.tables.map(t => {
    const count = g.nodes.filter(n => n.table === t).length;
    return `<div><span class="swatch" style="background:${g.color.get(t)}"></span>${esc(t)} <span class="hint">(${count})</span></div>`;
  }).join("") || '<p class="hint">No tables with records.</p>';
}

const list = items => items.length
  ? `<ul>${items.map(i => `<li>${esc(i)}</li>`).join("")}</ul>`
  : '<p class="hint">none</p>';
const fields = data => `<h2>Fields</h2><pre>${esc(JSON.stringify(data, null, 2))}</pre>`;

function nodeDetails(g, n) {
  const out = g.edges.filter(e => e.from === n.id).map(e => `${e.label} → ${g.nameOf(e.to)}`);
  const inc = g.edges.filter(e => e.to === n.id).map(e => `${g.nameOf(e.from)} → ${e.label}`);
  return `
    <p><strong>${esc(displayName(n))}</strong><br><code>${esc(n.id)}</code></p>
    <h2>Outgoing</h2>${list(out)}
    <h2>Incoming</h2>${list(inc)}
    ${fields(n.data)}`;
}

function edgeDetails(g, e) {
  return `
    <p><strong>${esc(g.nameOf(e.from))} → ${esc(e.label)} → ${esc(g.nameOf(e.to))}</strong></p>
    <p class="hint">Relation record <code>${esc(e.id)}</code></p>
    ${fields(e.data)}`;
}

function showSelection(g, selection) {
  const details = byId("details");
  if (selection.nodes.length) {
    details.innerHTML = nodeDetails(g, g.nodeById.get(selection.nodes[0]));
  } else if (selection.edges.length) {
    details.innerHTML = edgeDetails(g, g.edgeById.get(selection.edges[0]));
  } else {
    details.innerHTML = '<p class="hint">Nothing selected.</p>';
  }
}

// ---- Graph ----

const NETWORK_OPTIONS = {
  nodes: { shape: "dot", size: 14, font: { size: 14, color: NODE_FONT_COLOR } },
  edges: {
    arrows: "to",
    color: { color: EDGE_COLOR, highlight: "#555", hover: "#555" },
    font: { size: 11, color: EDGE_FONT_COLOR, strokeWidth: 0, align: "top" },
    smooth: { type: "dynamic" },
  },
  physics: { solver: "forceAtlas2Based", stabilization: { iterations: 200 } },
  interaction: { hover: true },
};

// Returns the vis-network instance plus its node and edge data sets,
// which the hover and menu features update.
function drawNetwork(g) {
  const nodes = new vis.DataSet(g.nodes.map(n => ({
    id: n.id,
    label: displayName(n),
    title: n.id,
    color: g.color.get(n.table),
  })));
  const edges = new vis.DataSet(g.edges.map(e => ({
    id: e.id,
    from: e.from,
    to: e.to,
    label: e.label,
  })));
  const network = new vis.Network(byId("graph"), { nodes, edges }, NETWORK_OPTIONS);
  return { network, nodes, edges, hidden: new Set() };
}

// ---- Hover highlight ----

// While a record is hovered, it and its direct neighbours keep their colours
// and labels; everything else fades. Hovering nothing restores all.
function highlight(g, view, nodeId) {
  const near = nodeId ? new Set([nodeId, ...view.network.getConnectedNodes(nodeId)]) : null;
  const nearEdges = nodeId ? new Set(view.network.getConnectedEdges(nodeId)) : null;

  view.nodes.update(g.nodes.map(n => {
    const lit = !near || near.has(n.id);
    return { id: n.id, color: lit ? g.color.get(n.table) : FADED, font: { color: lit ? NODE_FONT_COLOR : INVISIBLE } };
  }));
  view.edges.update(g.edges.map(e => {
    const lit = !nearEdges || nearEdges.has(e.id);
    return { id: e.id, color: { color: lit ? EDGE_COLOR : FADED }, font: { color: lit ? EDGE_FONT_COLOR : INVISIBLE } };
  }));
}

function setupHover(g, view) {
  view.network.on("hoverNode", ({ node }) => highlight(g, view, node));
  view.network.on("blurNode", () => highlight(g, view, null));
}

// ---- Record menu (right-click) ----

const MENU_ACTIONS = [
  { id: "inspect", label: "Inspect", run: (g, view, id) => {
      view.network.selectNodes([id]);
      showSelection(g, { nodes: [id], edges: [] });
    } },
  { id: "hide", label: "Hide record", run: (g, view, id) => {
      view.hidden.add(id);
      view.nodes.update({ id, hidden: true });
      highlight(g, view, null);
      renderHidden(g, view);
    } },
  { id: "focus", label: "Focus record", run: (g, view, id) => {
      view.network.selectNodes([id]);
      view.network.focus(id, { scale: 1.5, animation: { duration: 500 } });
      showSelection(g, { nodes: [id], edges: [] });
    } },
  { id: "copy", label: "Copy record id", run: (g, view, id) => navigator.clipboard.writeText(id) },
];

function openMenu(g, view, nodeId, x, y) {
  const n = g.nodeById.get(nodeId);
  const menu = byId("menu");
  menu.innerHTML = `
    <div class="title"><span class="swatch" style="background:${g.color.get(n.table)}"></span>${esc(n.id)}</div>
    ${MENU_ACTIONS.map(a => `<button data-action="${a.id}">${esc(a.label)}</button>`).join("")}`;
  menu.onclick = e => {
    const action = MENU_ACTIONS.find(a => a.id === e.target.dataset.action);
    if (!action) return;
    closeMenu();
    action.run(g, view, nodeId);
  };
  menu.hidden = false;
  // Keep the menu inside the window.
  const { width, height } = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(x, window.innerWidth - width - 8)}px`;
  menu.style.top = `${Math.min(y, window.innerHeight - height - 8)}px`;
}

function closeMenu() {
  byId("menu").hidden = true;
}

function setupMenu(g, view) {
  const container = byId("graph");
  view.network.on("oncontext", params => {
    params.event.preventDefault();
    const nodeId = view.network.getNodeAt(params.pointer.DOM);
    if (!nodeId) return closeMenu();
    const rect = container.getBoundingClientRect();
    openMenu(g, view, nodeId, rect.left + params.pointer.DOM.x, rect.top + params.pointer.DOM.y);
  });
  // Any click elsewhere, a drag, or Escape closes the menu.
  document.addEventListener("click", e => { if (!byId("menu").contains(e.target)) closeMenu(); });
  view.network.on("dragStart", closeMenu);
  document.addEventListener("keydown", e => { if (e.key === "Escape") closeMenu(); });
}

// Shows how many records are hidden, with a way to bring them back.
function renderHidden(g, view) {
  const el = byId("hidden");
  if (!view.hidden.size) { el.innerHTML = ""; return; }
  el.innerHTML = `<p class="hint">${plural(view.hidden.size, "record")} hidden · <button class="link" id="show-all">Show all</button></p>`;
  byId("show-all").onclick = () => {
    view.nodes.update([...view.hidden].map(id => ({ id, hidden: false })));
    view.hidden.clear();
    renderHidden(g, view);
  };
}

// ---- Start ----

async function main() {
  const g = prepare(await fetchGraph());
  renderSummary(g);
  renderLegend(g);
  const view = drawNetwork(g);
  view.network.on("click", selection => showSelection(g, selection));
  setupHover(g, view);
  setupMenu(g, view);
}

main().catch(e => { byId("error").textContent = "Could not load graph: " + e.message; });
