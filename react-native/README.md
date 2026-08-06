# rn-anydoc

Tula's universal document converter: `.docx`, `.doc`, `.xlsx`, `.pptx`,
`.rtf`, `.odt/.ods/.odp`, `.epub` → DocumentIR v2, natively, off the JS
thread. Wraps the `anydoc` Rust core (this repo) through a
[Nitro](https://nitro.margelo.com) HybridObject.

Measured on a Galaxy A05s: a 949kB thesis converts in ~220ms (~30× the JS
converter); a 16MB report in ~520ms. Both currently fall back to a file
card in the app.

## Contract

`convertDocumentToIr(bytes)` mirrors `rn-docx-ir.convertDocxToIr` exactly:
it resolves to `{status:'ok', ir, assets}` or `{status:'fallback', reason,
detail}` and **never rejects for document reasons** — the Rust side maps
even its own panics to a fallback. A rejection means the native module is
broken; callers should fall back to the JS converter. PDF is refused by
design (`unsupported-format`): the platform viewer owns it.

One buffer crosses the bridge per conversion:
`[u32 LE json len][IR JSON][asset bytes]`, sliced zero-copy on the JS side.

## Building

```sh
npm install            # dev deps (nitrogen)
npx nitrogen           # regenerate after touching src/specs/
./scripts/build-rust.sh  # Rust static libs -> android/libs/<abi>/ (gitignored)
```

## Integrating into the app (Phase 3 — not wired yet)

1. `"rn-anydoc": "link:../anydoc/react-native"` (or vendor the folder) +
   gradle `include ':rn-anydoc'`.
2. `documentPipeline.ts` routes non-docx formats here first; docx stays on
   `rn-docx-ir` until the A/B on the fixture corpus passes.
3. ABIs: arm64-v8a + armeabi-v7a. x86 emulators get no native converter —
   route to the JS fallback there.
4. iOS: `nitrogen/generated/ios` + a podspec exist but are untested —
   blocked on `ios-verification` landing first.

## Verification status

- Rust core: 194 tests, fuzzed per format upstream; FFI contract validated
  from Python (byte-exact offsets) and on-device (Galaxy A05s).
- C++ glue: compiles clean against Nitro 0.36.1 headers + RN jsi
  (`clang -fsyntax-only`); not yet built inside a host app.
- JS wrapper: typechecked; Hermes-safe (no TextDecoder).
