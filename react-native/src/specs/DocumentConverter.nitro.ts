import type { HybridObject } from 'react-native-nitro-modules'

/**
 * The one native call: document bytes in, one result buffer out, converted
 * on Nitro's thread pool so the JS thread never blocks.
 *
 * Result buffer layout (see anydoc/mobile/src/lib.rs, the single source of
 * truth): [u32 LE json length][DocumentIR JSON, UTF-8][asset bytes]. The
 * JSON's `assets` array carries each asset's offset/length into the trailing
 * blob. The call NEVER rejects for document reasons — every malformed,
 * encrypted or hostile input resolves to a {"status":"fallback"} payload.
 */
export interface DocumentConverter
  extends HybridObject<{ ios: 'c++'; android: 'c++' }> {
  convert(document: ArrayBuffer): Promise<ArrayBuffer>
}
