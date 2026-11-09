// The topics of the help window: short answers to the questions people ask
// about Ferry, each with the part of the documentation that says more.
// No DOM here, so scripts/check-frontend.mjs can check that every `doc`
// names a heading that exists (the texts are help_<id>_q and help_<id>_a).

/** The project website (docs built from the repository, see BUILDING.md). */
export const SITE = 'https://vp859.github.io/rust-quickshare/';

export const TOPICS = [
  { id: 'send', doc: 'README.md#quick-start' },
  { id: 'pair', doc: 'README.md#local-network-and-pairing' },
  { id: 'browser', doc: 'README.md#browser-sessions-no-app-on-the-other-device' },
  { id: 'apple', doc: 'README.md#localsend-compatibility-mode' },
  { id: 'internet', doc: 'README.md#network-modes' },
  { id: 'trouble', doc: 'README.md#faq' },
  { id: 'privacy', doc: 'docs/SECURITY-MODEL.md#what-each-party-can-see' },
];

/** The website address of a `doc` (`README.md` is the home page, `docs/X.md` is `X.html`). */
export function docUrl(doc) {
  const [file, anchor] = doc.split('#');
  const page = file === 'README.md' ? 'index.html' : file.replace(/^docs\//, '').replace(/\.md$/, '.html');
  return `${SITE}${page}#${anchor}`;
}
