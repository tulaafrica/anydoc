# react-native-anydoc

[![npm](https://img.shields.io/npm/v/react-native-anydoc.svg)](https://www.npmjs.com/package/react-native-anydoc)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Any office document → structured, styled JSON — converted **on the device,
in Rust, off the JS thread**.

`.docx` `.doc` `.pptx` `.ppt` `.xlsx` `.odt` `.ods` `.odp` `.rtf` `.epub`
in; **DocumentIR** out: paragraphs, headings, lists, tables, images, fonts,
colors, review comments, and typed chart data, ready to render with your
own React Native components. No server, no WebView.

Born inside [Tula](https://github.com/tulaafrica), an end-to-end encrypted
chat app, where documents must be converted on the sender's device because
no server ever holds plaintext.

```sh
npm install react-native-anydoc react-native-nitro-modules
```

**→ Full package docs: [react-native/README.md](react-native/README.md)** —
install (Android is automatic; iOS grabs one prebuilt XCFramework from
[Releases](https://github.com/tulaafrica/anydoc/releases)), the API and its
never-rejects contract, and the complete DocumentIR tour.

## Performance

Real documents, real phones, measured end-to-end inside a running app:

| Document | Size | Galaxy A05s (~$120) | iPhone 16 Pro |
|---|---|---|---|
| 130-page thesis, 72 comments, 16 images (.docx) | 949 kB | 226 ms | 43 ms |
| Slide deck, 8 slides (.pptx) | 200 kB | 17 ms | 3 ms |
| 9-sheet workbook (.xlsx) | 49 kB | ~30 ms | 7 ms |

The JS converter this replaced took **6.9 seconds** for the thesis on the
same phone. Full table and methodology in the
[package README](react-native/README.md#performance).

## What's in this repo

- **[`react-native/`](react-native/)** — the npm package: Nitro Modules
  bridge, prebuilt-binary plumbing, the docs.
- **[`mobile/`](mobile/)** — the mobile FFI crate: a C ABI over the engine
  that emits DocumentIR JSON and hands image bytes across zero-copy.
- **[`src/`](src/)** — the conversion engine (see below).

## The engine

This is a fork of [anydoc](https://github.com/firecrawl/anydoc), the
excellent MIT-licensed Rust document parser built by
[Firecrawl](https://firecrawl.dev) — if you want **Markdown** out of office
documents (CLI, Node.js, Python, WASM), use upstream; it's built for
exactly that.

This fork adds what a *renderer* needs and Markdown can't carry:

- A **presentation model** — fonts, sizes, colors, highlights, alignment,
  indents, spacing, table borders and cell shading, resolved through each
  format's style cascade.
- **Review comments**, anchored to the exact runs they cover.
- **Typed chart data** — kind, categories, numeric series — instead of a
  flattened table, so apps can draw real charts.
- Slide/sheet **pagination** and the **mobile FFI + React Native bridge**.

The fork tracks upstream closely: our Markdown output is kept
**byte-identical** to upstream's (verified by the original snapshot suite
on every change), so upstream merges stay routine and every upstream
parsing fix lands here.

## License

MIT. The anydoc engine is © Sideguide Technologies Inc. (Firecrawl), also
MIT — see [LICENSE](LICENSE). Presentation model, mobile FFI and the React
Native package by [Tula](https://github.com/tulaafrica).
