/**
 * rn-anydoc: any office document -> DocumentIR, natively, off the JS thread.
 *
 * The API mirrors rn-docx-ir's `convertDocxToIr` contract exactly - same
 * result shape, same never-throws-for-document-reasons promise - so the
 * document pipeline can route by format without knowing which converter
 * produced the IR.
 */

import { NitroModules } from 'react-native-nitro-modules'
import type { DocumentConverter } from './specs/DocumentConverter.nitro'

/** One extracted asset (image bytes referenced by the IR's assetRefs). */
export interface ConvertedAsset {
  assetRef: string
  contentType: string | null
  bytes: ArrayBuffer
}

export type ConvertResult =
  | {
      status: 'ok'
      /** DocumentIR v2 - identical shape to rn-docx-ir's output. */
      ir: unknown
      assets: ConvertedAsset[]
      warnings: string[]
    }
  | { status: 'fallback'; reason: string; detail: string }

let converter: DocumentConverter | null = null

function getConverter(): DocumentConverter {
  if (converter == null) {
    converter = NitroModules.createHybridObject<DocumentConverter>('DocumentConverter')
  }
  return converter
}

/**
 * Convert document bytes to DocumentIR.
 *
 * NEVER rejects for document reasons - corrupt, encrypted, unsupported and
 * hostile inputs all resolve to `{status: 'fallback'}` (the Rust side maps
 * even its own panics to that). A rejection here means the native module
 * itself is broken, and the caller should fall back to the JS converter.
 */
export async function convertDocumentToIr(
  bytes: ArrayBuffer | Uint8Array,
): Promise<ConvertResult> {
  const input =
    bytes instanceof Uint8Array
      ? bytes.byteLength === bytes.buffer.byteLength && bytes.byteOffset === 0
        ? (bytes.buffer as ArrayBuffer)
        : bytes.slice().buffer
      : bytes

  const output = await getConverter().convert(input)
  return parseResultBuffer(output)
}

/**
 * The Rust side's buffer layout (anydoc/mobile/src/lib.rs):
 * [u32 LE json length][DocumentIR JSON, UTF-8][asset bytes], with each
 * asset's offset/length into the trailing blob declared in the JSON.
 */
function parseResultBuffer(buffer: ArrayBuffer): ConvertResult {
  const view = new DataView(buffer)
  const jsonLength = view.getUint32(0, true)
  const json = utf8Decode(new Uint8Array(buffer, 4, jsonLength))
  const parsed = JSON.parse(json)

  if (parsed.status !== 'ok') {
    return {
      status: 'fallback',
      reason: parsed.reason ?? 'parse-error',
      detail: parsed.detail ?? '',
    }
  }

  const blobStart = 4 + jsonLength
  const assets: ConvertedAsset[] = (parsed.assets ?? []).map(
    (asset: { assetRef: string; contentType?: string; offset: number; length: number }) => ({
      assetRef: asset.assetRef,
      contentType: asset.contentType ?? null,
      bytes: buffer.slice(blobStart + asset.offset, blobStart + asset.offset + asset.length),
    }),
  )

  return { status: 'ok', ir: parsed.ir, assets, warnings: [] }
}

/**
 * UTF-8 decode without TextDecoder, which Hermes does not have. Chunked
 * String.fromCharCode.apply keeps call-stack use bounded on large documents.
 */
function utf8Decode(bytes: Uint8Array): string {
  const codes: number[] = []
  let i = 0
  // Indexing is bounds-safe by construction (the Rust side always emits
  // complete UTF-8), but the app compiles with noUncheckedIndexedAccess.
  const at = (index: number): number => bytes[index] ?? 0
  while (i < bytes.length) {
    const b = at(i++)
    let c: number
    if (b < 0x80) {
      c = b
    } else if (b < 0xe0) {
      c = ((b & 0x1f) << 6) | (at(i++) & 0x3f)
    } else if (b < 0xf0) {
      c = ((b & 0x0f) << 12) | ((at(i++) & 0x3f) << 6) | (at(i++) & 0x3f)
    } else {
      c =
        (((b & 0x07) << 18) | ((at(i++) & 0x3f) << 12) | ((at(i++) & 0x3f) << 6) | (at(i++) & 0x3f)) -
        0x10000
      codes.push(0xd800 | (c >> 10), 0xdc00 | (c & 0x3ff))
      continue
    }
    codes.push(c)
  }
  let out = ''
  for (let start = 0; start < codes.length; start += 8192) {
    out += String.fromCharCode.apply(null, codes.slice(start, start + 8192))
  }
  return out
}

export type { DocumentConverter }
