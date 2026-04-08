import { sha256Hex } from "./crypto";

export function snapshotObjectKey(vaultId: string, snapshotId: string): string {
  return `snapshots/${vaultId}/${snapshotId}.json`;
}

export function commitObjectKey(
  vaultId: string,
  streamSeq: number,
  commitId: string,
): string {
  return `commits/${vaultId}/${streamSeq.toString().padStart(20, "0")}-${commitId}.json`;
}

export async function jsonPayloadDigest(payload: unknown): Promise<string> {
  return sha256Hex(JSON.stringify(payload));
}

