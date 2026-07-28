use axum::response::Html;

const PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>dax-rs</title>
<style>
  body { background-color: #fafafa; margin: 2rem; color: #222; }
  h1   { font-size: 1.2rem; margin: 0 0 0.25rem; }
  main { background-color: #fff; border-radius: 0.25rem; border-style: solid; border-width: 1px; border-color: rgba(0, 0, 0, 0.2); }
  button { font-family: monospace; font-size: 0.8rem; padding: 0.2rem 0.6rem; cursor: pointer; background: blue; color: white; border: none; padding: 0.5rem; border-radius: 0.25rem; font-weight: bold; }
  button.inline { border: none; background: none; color: blue; }
  button.inline:hover { background: #ddd; }
  #reload { margin-bottom: 1rem; }
  table { border-collapse: collapse; width: 100%; }
  th { text-align: left; border-bottom: 1px solid #ddd; padding: 0.55rem 0.75rem; font-size: 0.8rem; white-space: nowrap; }
  td { padding: 0.55rem 0.75rem; border-bottom: 1px solid #ddd; font-size: 0.85rem; }
  tr:hover td { background: #f5f5f5; }
  .dim { color: #999; }
  .err { color: #c00; }
</style>
</head>
<body>
<h1>dax-rs</h1>
<button id="reload" onclick="load()">&#8635; Reload</button>
<main>
<table>
  <thead><tr>
    <th>Model</th>
    <th>Tables</th>
    <th>Measures</th>
    <th>Last Refreshed</th>
    <th>Last Schema Update</th>
    <th></th>
  </tr></thead>
  <tbody id="body"></tbody>
</table>
</main>
<script>
function esc(s) {
  return String(s ?? '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;');
}

const tbody = document.getElementById('body');

async function load() {
  tbody.innerHTML = '<tr><td colspan="6" class="dim">Loading…</td></tr>';
  try {
    const models = await fetch('/models').then(r => r.json());
    if (!models.length) {
      tbody.innerHTML = '<tr><td colspan="6" class="dim">No models loaded.</td></tr>';
      return;
    }
    const rows = await Promise.all(models.map(async m => {
      const [detail, tables, measures] = await Promise.all([
        fetch('/models/' + encodeURIComponent(m.id)).then(r => r.json()),
        fetch('/models/' + encodeURIComponent(m.id) + '/tables').then(r => r.json()),
        fetch('/models/' + encodeURIComponent(m.id) + '/measures').then(r => r.json()),
      ]);
      return { id: m.id, name: m.name, detail, tableCount: tables.length, measureCount: measures.length };
    }));
    // data-id is HTML-attribute-escaped via esc(); the refresh button's click
    // handler is attached separately below rather than via an inline onclick,
    // so model names/ids can never break out of the attribute they sit in.
    tbody.innerHTML = rows.map(r => '<tr>' +
      '<td>' + esc(r.name) + '</td>' +
      '<td>' + r.tableCount + '</td>' +
      '<td>' + r.measureCount + '</td>' +
      '<td>' + esc(r.detail.last_refreshed) + '</td>' +
      '<td>' + esc(r.detail.last_schema_update) + '</td>' +
      '<td>' +
        '<button class="refresh-btn inline" data-id="' + esc(r.id) + '">Refresh</button> ' +
        '<button class="reload-model-btn inline" data-id="' + esc(r.id) + '">Reload Model</button>' +
      '</td>' +
    '</tr>').join('');
  } catch (e) {
    tbody.innerHTML = '<tr><td colspan="6" class="err">' + esc(e) + '</td></tr>';
  }
}

async function refresh(id, btn) {
  btn.disabled = true;
  btn.textContent = '…';
  const res = await fetch('/models/' + encodeURIComponent(id) + '/refreshdata', { method: 'POST' })
    .catch(() => null);
  if (res && (res.ok || res.status === 204)) {
    btn.textContent = '✓';
    setTimeout(load, 300);
  } else {
    btn.textContent = '✗';
    btn.style.color = '#c00';
    btn.disabled = false;
  }
}

async function reloadModel(id, btn) {
  btn.disabled = true;
  btn.textContent = '…';
  const res = await fetch('/models/' + encodeURIComponent(id) + '/reloadmodel', { method: 'POST' })
    .catch(() => null);
  if (res && (res.ok || res.status === 204)) {
    btn.textContent = '✓';
    setTimeout(load, 300);
  } else {
    btn.textContent = '✗';
    btn.style.color = '#c00';
    btn.disabled = false;
  }
}

tbody.addEventListener('click', e => {
  const refreshBtn = e.target.closest('button.refresh-btn');
  if (refreshBtn) refresh(refreshBtn.dataset.id, refreshBtn);
  const reloadBtn = e.target.closest('button.reload-model-btn');
  if (reloadBtn) reloadModel(reloadBtn.dataset.id, reloadBtn);
});

load();
</script>
</body>
</html>"#;

pub async fn dashboard() -> Html<&'static str> {
    Html(PAGE)
}
