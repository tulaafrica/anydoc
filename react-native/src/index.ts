/**
 * rn-anydoc: any office document -> DocumentIR, natively, off the JS thread.
 *
 * The API mirrors rn-docx-ir's `convertDocxToIr` contract exactly - same
 * result shape, same never-throws-for-document-reasons promise - so the
 * document pipeline can route by format without knowing which converter
 * produced the IR.
 */

import { NitroModules } from 'react-native-nitro-modules'
import type { DocumentConverter, NativeConvertOutput } from './specs/DocumentConverter.nitro'

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
  return parseResult(output)
}

/**
 * The native layer already split the Rust buffer: the result JSON arrives as
 * a string (UTF-8 -> UTF-16 done in C++), and `assets` is the raw trailing
 * blob with each asset's offset/length declared in the JSON.
 */
function parseResult(output: NativeConvertOutput): ConvertResult {
  const parsed = JSON.parse(output.json)

  if (parsed.status !== 'ok') {
    return {
      status: 'fallback',
      reason: parsed.reason ?? 'parse-error',
      detail: parsed.detail ?? '',
    }
  }

  const blob = output.assets
  const assets: ConvertedAsset[] = (parsed.assets ?? []).map(
    (asset: { assetRef: string; contentType?: string; offset: number; length: number }) => ({
      assetRef: asset.assetRef,
      contentType: asset.contentType ?? null,
      bytes: blob.slice(asset.offset, asset.offset + asset.length),
    }),
  )

  return { status: 'ok', ir: parsed.ir, assets, warnings: [] }
}

export type { DocumentConverter }
