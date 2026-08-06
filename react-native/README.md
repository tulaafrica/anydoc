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
no server ever holds plaintext. Measured on a Galaxy A05s (a $120 phone): a
949kB, 130-page thesis with 72 comments and 16 images converts end-to-end
in **~226ms** — about 30× the JS-based converter it replaced. A 16MB report
takes ~520ms.

> **Status: Android, 0.x.** iOS bindings (`nitrogen/generated/ios` + Nitro
> podspec) exist but are not yet built or verified — iOS support is next.
> Upstream anydoc moves fast and we aim to track it; the fork keeps its
> Markdown output byte-identical to upstream so merges stay routine.

## Install

```sh
npm install react-native-anydoc react-native-nitro-modules
```

Autolinking does the rest. On first Android build, gradle downloads the
prebuilt Rust core (~80MB zip) from this repo's GitHub Releases; to build
it from source instead (Rust + Android NDK required):

```sh
npx nitrogen                                # if you touched src/specs/
./node_modules/react-native-anydoc/scripts/build-rust.sh
```

ABIs: `arm64-v8a`, `armeabi-v7a`, `x86_64`. (32-bit x86 emulators get no
native slice — handle the rejection as shown below.)

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
*rejection* means the native module itself is missing or broken (32-bit
x86 emulator, bad install) — catch it and route to whatever fallback your
app has. PDF is refused by design (`unsupported-format`): the platform's
own viewer renders PDFs better than any re-conversion.

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
    "blocks": [
      { "type": "heading", "level": 1, "runs": [{ "text": "Title", "bold": true, "fontFamily": "Calibri", "fontSize": 21 }] },
      { "type": "paragraph", "align": "justify", "runs": [{ "text": "Body…", "commentIds": ["0"] }] },
      { "type": "table", "columnWidths": [120, 240], "rows": [[{ "paragraphs": [[{ "text": "cell" }]], "background": "#DDEBF7" }]] },
      { "type": "image", "assetRef": "word/media/image1.png", "width": 320, "height": 200 }
    ]
  }],
  "comments": [{ "id": "0", "author": "Reviewer", "date": "…", "blocks": [] }]
}
```

Sizes are in pixels, colors are `#RRGGBB`, and every presentation field is
optional — absent means "renderer's default", which keeps real-world IR
small. A `.pptx` arrives as one IR page per slide.

## License

MIT. The anydoc engine is © Sideguide Technologies Inc. (Firecrawl), also
MIT — see [LICENSE](../LICENSE). Presentation model, mobile FFI and this
React Native package by [Tula](https://github.com/tulaafrica).
