// The accessibility audit shared by the tests of the GUI and of the browser
// session page: it runs in the page (`page.evaluate(audit, selector)`).

/**
 * Problems of the visible part of the page (or of `rootSelector`), found in
 * the browser: controls without a name, fields without a label, duplicate IDs,
 * dialogs without a title, and text with too little contrast.
 */
export function audit(rootSelector) {
  const root = rootSelector ? document.querySelector(rootSelector) : document.body;
  const problems = [];
  const visible = (e) => {
    if (!(e instanceof Element)) return false;
    const s = getComputedStyle(e);
    if (s.visibility === 'hidden' || s.display === 'none' || Number(s.opacity) === 0) return false;
    const r = e.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  };
  const whatIs = (e) => e.id ? `#${e.id}` : `${e.tagName.toLowerCase()}.${[...e.classList].join('.')} "${(e.textContent || '').trim().slice(0, 40)}"`;

  // Accessible names (a simplified version of the accname algorithm).
  const nameOf = (e) => {
    const labelled = e.getAttribute('aria-labelledby');
    if (labelled) return labelled.split(/\s+/).map(id => document.getElementById(id)?.textContent ?? '').join(' ').trim();
    if (e.getAttribute('aria-label')) return e.getAttribute('aria-label').trim();
    if (e.id) {
      const label = document.querySelector(`label[for="${CSS.escape(e.id)}"]`);
      if (label) return label.textContent.trim();
    }
    const wrapping = e.closest('label');
    if (wrapping && wrapping !== e) return wrapping.textContent.trim();
    // innerText: only what is rendered (a label hidden on small screens is no name).
    // biome-ignore lint/performance/useDomNodeTextContent: the rendered text is exactly what is checked here.
    if (['BUTTON', 'A'].includes(e.tagName) || e.getAttribute('role') === 'tab') return (e.innerText || '').trim() || (e.getAttribute('title') || '').trim();
    return (e.getAttribute('title') || e.getAttribute('placeholder') || '').trim();
  };
  for (const e of root.querySelectorAll('button, a[href], input:not([type=hidden]), select, textarea, [role=tab], [role=switch]')) {
    // A switch is a hidden checkbox inside a visible label: check it even when hidden.
    const shown = visible(e) || (e.type === 'checkbox' && visible(e.closest('label') ?? e));
    if (!shown) continue;
    if (!nameOf(e)) problems.push(`no accessible name: ${whatIs(e)}`);
  }

  const ids = new Map();
  for (const e of document.querySelectorAll('[id]')) ids.set(e.id, (ids.get(e.id) ?? 0) + 1);
  for (const [id, n] of ids) if (n > 1) problems.push(`duplicate id: ${id} (${n}×)`);

  for (const d of document.querySelectorAll('.modal-overlay')) {
    const role = d.getAttribute('role');
    if (role !== 'dialog' && role !== 'alertdialog') problems.push(`dialog without role: #${d.id}`);
    if (d.getAttribute('aria-modal') !== 'true') problems.push(`dialog not modal: #${d.id}`);
    const title = d.getAttribute('aria-labelledby');
    if (!title || !document.getElementById(title)) problems.push(`dialog without a title: #${d.id}`);
  }
  for (const svg of root.querySelectorAll('svg.i')) {
    const button = svg.closest('button, a');
    if (button && visible(button) && !nameOf(button)) problems.push(`icon-only control without name: ${whatIs(button)}`);
  }

  // Contrast of text against what is behind it.
  const parse = (c) => {
    const m = c.match(/rgba?\(([^)]+)\)/);
    if (!m) return null;
    const [r, g, b, a = 1] = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
    return [r, g, b, a];
  };
  const over = (top, bottom) => {
    const a = top[3];
    return [0, 1, 2].map(i => top[i] * a + bottom[i] * (1 - a)).concat(1);
  };
  const background = (e) => {
    const layers = [];
    for (let n = e; n; n = n.parentElement) {
      const c = parse(getComputedStyle(n).backgroundColor);
      if (c && c[3] > 0) layers.push(c);
      if (c && c[3] >= 1) break;
    }
    let color = [255, 255, 255, 1];
    for (const layer of layers.reverse()) color = over(layer, color);
    return color;
  };
  const luminance = ([r, g, b]) => {
    const lin = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  };
  const ratio = (a, b) => {
    const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
    return (x + 0.05) / (y + 0.05);
  };
  for (const e of root.querySelectorAll('*')) {
    if (!visible(e) || e.closest('svg')) continue;
    const ownText = [...e.childNodes].some(n => n.nodeType === Node.TEXT_NODE && n.textContent.trim());
    const field = ['INPUT', 'TEXTAREA'].includes(e.tagName) && e.value;
    if (!ownText && !field) continue;
    const s = getComputedStyle(e);
    // Disabled controls are exempt (WCAG 1.4.3).
    if (e.closest('button:disabled, input:disabled')) continue;
    const bg = background(e);
    const fg = over(parse(s.color), bg);
    const size = parseFloat(s.fontSize);
    const large = size >= 24 || (size >= 18.66 && Number(s.fontWeight) >= 700);
    const need = large ? 3 : 4.5;
    const got = ratio(fg, bg);
    if (got < need) problems.push(`contrast ${got.toFixed(2)} < ${need}: ${whatIs(e)} (${s.color} on rgb(${bg.slice(0, 3).map(Math.round)}))`);
  }
  return [...new Set(problems)];
}

