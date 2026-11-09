# @noble/ciphers (vendored)

ChaCha20-Poly1305 for the encryption of browser sessions (`../seal.js`, `src/web/seal.rs`):
browsers offer WebCrypto only to HTTPS pages, and a session runs over plain HTTP.

These are unchanged copies of `chacha.js`, `_arx.js`, `_poly1305.js`, `utils.js` and `LICENSE` (MIT)
of [@noble/ciphers](https://github.com/paulmillr/noble-ciphers) **2.4.0**, the version
`package-lock.json` pins. `npm run check:ui` fails when they differ from the installed package.

To update: change the version in `package.json`, run `npm install`, copy the files again
(`cp node_modules/@noble/ciphers/{chacha,_arx,_poly1305,utils}.js node_modules/@noble/ciphers/LICENSE src/web/page/vendor/`)
and run the tests (`cargo test --test web`, `npm run test:ui`).
