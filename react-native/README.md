# react-native-anydoc

Any office document → structured, styled JSON — converted **on the device,
in Rust, off the JS thread**.

`.docx` `.doc` `.pptx` `.ppt` `.xlsx` `.odt` `.ods` `.odp` `.rtf` `.epub`
in; **DocumentIR** out: paragraphs, headings, lists, tables (borders,
column widths, cell shading), images, fonts, sizes, colors, highlights and
document comments, ready to render with your own components. Built on the
[anydoc](https://github.com/firecrawl/anydoc) Rust engine by Firecrawl
(MIT), extended with a presentation model in the
[tulaafrica/anydoc](https://github.com/tulaafrica/anydoc) fork, and bridged
with [Nitro Modules](https://nitro.margelo.com).

Born inside [Tula](https://github.com/tulaafrica), an end-to-end encrypted
chat app, where documents must be converted on the sender's device because
no server ever holds plaintext.

## Performance

Real documents, real phones, measured end-to-end **inside a running app**
(JS call → Nitro thread pool → Rust → result parsed back in JS). Not
microbenchmarks.

| Document | Size | Galaxy A05s (~$120) | iPhone 16 Pro |
|---|---|---|---|
| 130-page thesis, 72 comments, 16 images (.docx) | 949 kB | 226 ms | 43 ms |
| Architecture doc, 27 tables (.docx) | 37 kB | 47 ms | 15 ms |
| Slide deck, 8 slides (.pptx) | 200 kB | 17 ms | 3 ms |
| Legacy Word 97 file (.doc) | 64 kB | 4 ms | <1 ms |
| 9-sheet workbook (.xlsx) | 49 kB | ~30 ms | 7 ms |
| 16 MB report (.docx) | 16 MB | ~520 ms | — |
| 100 kB of random bytes | 100 kB | clean fallback | clean fallback |

Android numbers are from a dev-mode Hermes build on a Samsung Galaxy A05s
(the low-end reference device — measured there deliberately); iPhone
numbers are from a Release build on an iPhone 16 Pro. The JS converter
this replaced took **6.9 seconds** for the thesis on the same A05s; over a
30-document corpus the Rust core was ~29× faster overall, with text output
hash-identical on every Word-produced document.

## Size

What the package actually adds to an app:

| | |
|---|---|
| JS (npm tarball) | 44 kB unpacked |
| Rust core, linked into your app | **~2.0 MB per ABI**, ~1 MB compressed in the store download |

Don't be alarmed by the release assets (a ~36 MB Android zip, a ~23 MB
XCFramework zip): those are *static archives* — per-function sections, three
Android ABIs, device + simulator iOS slices. The linker keeps only what
your app actually calls. The core is built with `opt-level = "z"`, fat
LTO and stripped symbols; dead code is eliminated at link time, and the
PDF parser isn't compiled in at all (PDF is refused by design — see
below).

## Install

```sh
npm install react-native-anydoc react-native-nitro-modules
```

Autolinking handles registration on both platforms. The prebuilt Rust core
is too large for the npm tarball, so it ships as GitHub Release assets:

- **Android** — nothing to do: on first build, gradle downloads the static
  libraries (`arm64-v8a`, `armeabi-v7a`, `x86_64`) from this repo's
  Releases automatically. Override the source with
  `-PanydocLibsUrl=<url>`, or build from source (Rust + Android NDK):

  ```sh
  ./node_modules/react-native-anydoc/scripts/build-rust.sh
  ```

- **iOS** — also nothing to do: the package's `postinstall` downloads
  `AnydocCore.xcframework` from the matching release on macOS (skipped
  when it's already present, when not on macOS, or with
  `ANYDOC_SKIP_IOS_CORE=1`; point `ANYDOC_IOS_CORE_URL` at a mirror if
  GitHub is unreachable). Then `pod install` as usual. If the download was
  skipped or failed, run it by hand:

  ```sh
  node node_modules/react-native-anydoc/scripts/install-anydoc-core.js
  ```

  Or grab the zip from the
  [matching release](https://github.com/tulaafrica/anydoc/releases) and
  unzip it into `node_modules/react-native-anydoc/ios/` yourself — or
  build from source (Rust + Xcode):

  ```sh
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim
  ./node_modules/react-native-anydoc/scripts/build-rust-ios.sh
  ```

## Use

```ts
import { convertDocumentToIr } from 'react-native-anydoc'

const result = await convertDocumentToIr(bytes /* ArrayBuffer | Uint8Array */)

if (result.status === 'ok') {
  render(result.ir)                    // DocumentIR: pages -> blocks -> runs
  for (const asset of result.assets) { // images referenced by the IR
    save(asset.assetRef, asset.bytes)  //   (ArrayBuffer, zero-copy)
  }
} else {
  // 'fallback': the DOCUMENT can't be converted (corrupt, encrypted,
  // unsupported). Expected and safe - show a file card instead.
  console.log(result.reason, result.detail)
}
```

**The contract:** the promise **never rejects for document reasons** —
corrupt, encrypted, and hostile inputs all resolve to
`{status: 'fallback'}`; the Rust side maps even its own panics to that. A
*rejection* means the native module itself is missing or broken (an ABI
without a prebuilt slice, a bad install) — catch it and route to whatever
fallback your app has:

```ts
async function convert(bytes: Uint8Array) {
  try {
    return await convertDocumentToIr(bytes)
  } catch {
    // The MODULE is broken, not the document. Degrade, don't block:
    return { status: 'fallback', reason: 'native-module-missing', detail: '' } as const
  }
}
```

PDF is refused by design (`unsupported-format`): the platform's own viewer
renders PDFs better than any re-conversion.

Conversion runs on Nitro's thread pool; the JS thread never blocks. The
result JSON crosses the bridge as a native string (UTF-8→UTF-16 in C++),
and asset bytes arrive zero-copy — the GC's cleanup callback for the
buffer *is* the Rust deallocator.

## DocumentIR in one glance

```jsonc
{
  "version": 2,
  "sourceType": "docx",
  "pages": [{
    "pageIndex": 0,
    "name": "Asset Quality",   // spreadsheets: one NAMED page per sheet
    "blocks": [
      { "type": "heading", "level": 1, "runs": [{ "text": "Title", "bold": true, "fontFamily": "Calibri", "fontSize": 21 }] },
      { "type": "paragraph", "align": "justify", "runs": [{ "text": "Body…", "commentIds": ["0"] }] },
      { "type": "table", "columnWidths": [120, 240], "rows": [[{ "paragraphs": [[{ "text": "cell" }]], "background": "#DDEBF7" }]] },
      { "type": "image", "assetRef": "word/media/image1.png", "width": 320, "height": 200 },
      { "type": "chart", "kind": "pie", "title": "Share", "categories": ["A", "B"], "series": [{ "name": "S1", "values": [60, 40], "labels": ["60", "40"] }] }
    ]
  }],
  "comments": [{ "id": "0", "author": "Reviewer", "date": "…", "text": "Margin note." }]
}
```

The shape in detail:

- **Pages** — a `.pptx`/`.ppt`/`.odp` arrives as one page per slide; a
  multi-sheet `.xlsx`/`.ods` as one **named** page per sheet (the name is
  the sheet's tab). Word formats emit a single page (Word stores no page
  breaks — they are computed at layout time).
- **Charts** (`.xlsx`, and embedded charts in `.docx`/`.pptx`) arrive as a
  typed `chart` block placed after the owning sheet's cells — dedicated
  chart sheets included: `kind`
  (`bar`/`line`/`area`/`pie`/`doughnut`/`scatter`/`radar`/`other`), `title`,
  `axisTitle`, `categories`, and `series` (each with `name`, numeric
  `values` — `null` where the workbook cached a non-numeric point — and the
  cached display `labels`). Draw the kinds you support; for the rest,
  rebuild the classic titled categories × series data table from
  `categories` × `labels` — the block always carries enough to do so.
- **Equations** (Word OMML, ODF/EPUB MathML) arrive as their **LaTeX source**
  in runs flagged `math: true` — an inline formula sits among its prose, a
  displayed formula is a centered paragraph whose only run is math. The
  runs are italic too, so a renderer without a typesetter still shows them
  legibly; one with a typesetter (MathJax, KaTeX) has the source verbatim.
  **Checkboxes** (form controls, task lists) arrive as ☐ / ☑ glyphs.
- **Runs** carry the formatting: `bold`, `italic`, `underline`,
  `strikethrough`, `fontFamily`, `fontSize` (px), `color` (`#RRGGBB`),
  `highlightColor` (Word's 16-name enum), `verticalAlign`, `caps`, and
  `commentIds`.
- **Blocks** carry layout: `align`, `indent`, `spacing` (px), table
  `borders`/`columnWidths`, cell `background`/`colSpan`/`rowSpan`.
- **Comments** are document-level; the runs inside a comment's range carry
  its id in `commentIds`, so highlights and tap-to-open need no extra
  bookkeeping — and runs with different comment coverage are never merged.
- Every presentation field is **optional** — absent means "renderer's
  default", which keeps real-world IR small.
- **Images** never carry bytes in the IR; each `image` block has an
  `assetRef` into `result.assets`.

### Slides as PowerPoint draws them

A `.pptx` page also carries `slide`: the slide's fixed layout, with every
shape at its own position. The page's `blocks` still hold the same content
as flowing text, so a reader can offer both a slide view and a text view.

```jsonc
"slide": {
  "width": 1280, "height": 720,                  // px at 96 dpi; scale the whole slide by one factor
  "background": { "type": "solid", "color": "#0A0815" },
  "shapes": [                                    // paint order, back to front; groups are flattened
    {
      "x": 58, "y": 178, "width": 270, "height": 240,
      "rotation": 30, "flipH": true,             // optional
      "geometry": { "type": "roundRect", "radius": 0.032 },
      "fill": { "type": "solid", "color": "#1C1530" },
      "line": { "width": 1, "color": "#3B2A66", "dash": "dash", "tailEnd": "triangle" },
      "shadow": { "dx": 0, "dy": 4, "blur": 8, "color": "#00000059" },
      "text": {
        "insets": [10, 5, 10, 5],                // left, top, right, bottom
        "anchor": "middle",                      // omitted = top
        "paragraphs": [{
          "align": "center", "marginLeft": 24, "indent": -24,
          "lineSpacing": { "lines": 0.9 },       // or { "px": 20 }
          "spaceBefore": { "px": 6 }, "spaceAfter": { "lines": 0.2 },
          "bullet": { "text": "•", "color": "#7C3AED", "size": 1.1 },
          "endSize": 24,
          "runs": [{ "text": "Kenya", "fontSize": 24, "color": "#FFFFFF", "fontFamily": "Calibri", "bold": true }]
        }]
      }
    },
    { "x": 0, "y": 0, "width": 160, "height": 120, "geometry": { "type": "ellipse" },
      "image": { "assetRef": "image3.png", "crop": { "left": 0.25, "top": 0, "right": 0.25, "bottom": 0 } } },
    { "x": 260, "y": 400, "width": 200, "height": 80,
      "geometry": { "type": "path", "d": "M0 40 C50 0 50 80 100 40 …Z" }, "preset": "custom", "fill": { … } }
  ]
}
```

Everything is resolved before it reaches you: theme colours, inheritance
from the slide layout and master (their decoration is already in `shapes`),
PowerPoint's style references, table styles (including the built-in
default), autofit shrinking, numbering. Rendering rules a renderer must
follow to match PowerPoint:

- **Geometry** is `rect`, `roundRect` (`radius` is a fraction of the shorter
  side), `ellipse`, `line` (top-left to bottom-right; flips pick the other
  diagonal; it may have zero width or height and must still draw), or
  `path` - SVG path data in the shape's own px box, non-zero fill rule,
  `filled: false` for open strokes. Pictures are clipped to their geometry.
- **Flips mirror the outline, never the text.** Rotation turns both.
- **Colours** are `#RRGGBB`, or `#RRGGBBAA` when translucent.
- **Fills** are `solid`, `gradient` (`angle` in degrees, 90 = top to
  bottom; `radial`; `stops` with `offset` 0-1) or `image` (an `assetRef`
  stretched over the shape).
- **Line spacing** `{ "lines": n }` is a multiple of PowerPoint's single
  spacing, which is **1.2 × the font size**; `{ "px": n }` is absolute.
  Paragraph spacing in `lines` is relative to the paragraph's first run.
- **Bullets** sit in the hanging indent: the first line starts at
  `marginLeft + indent`, wrapped lines at `marginLeft`. A bullet with no
  `color`/`fontFamily` follows the paragraph's first run; `size` scales it.
- **Runs**: `baseline` raises (positive) or lowers the run by that fraction
  of its size, drawn at about two thirds of the size;
  `letterSpacing` is px; `caps: "all"` renders in capitals. A run whose
  text contains `\n` breaks the line there.
- **`wrap: false`** keeps each paragraph on one line, overflowing the box.
- **Tables** are positioned like shapes; `columns` are px widths, each row
  has a minimum `height`, and each cell names its grid `col` (cells covered
  by a merge are omitted) with its own `fill`, `borders`, `insets` and
  `anchor`.
- **Charts** carry the same typed data as a `chart` block; draw it inside
  the frame.
- **Fonts** are the author's names (`Calibri`, `Cambria`, `Georgia`). Text
  lands where PowerPoint puts it only with metric-compatible faces: Carlito
  for Calibri, Caladea for Cambria, Gelasio for Georgia, Arimo for Arial,
  Tinos for Times New Roman.

A slide whose content cannot all be placed (a producer that left out
positions) has no `slide`; show its `blocks` instead. Legacy `.ppt` and
`.odp` decks have no fixed layout yet.

## Upstream

Upstream anydoc moves fast and this fork tracks it: the fork's Markdown
output is kept byte-identical to upstream (verified by the original
snapshot suite on every change), so merges stay routine.

## License

MIT. The anydoc engine is © Sideguide Technologies Inc. (Firecrawl), also
MIT — see [LICENSE](../LICENSE). Presentation model, mobile FFI and this
React Native package by [Tula](https://github.com/tulaafrica).
