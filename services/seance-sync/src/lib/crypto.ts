const encoder = new TextEncoder();

export function nowTs(): number {
  return Math.floor(Date.now() / 1000);
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}

export function base64UrlEncode(bytes: Uint8Array): string {
  return bytesToBase64(bytes).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

export function base64UrlDecode(value: string): Uint8Array {
  const normalized = value.replace(/-/g, "+").replace(/_/g, "/");
  const padded = normalized + "=".repeat((4 - (normalized.length % 4 || 4)) % 4);
  return base64ToBytes(padded);
}

export async function sha256Hex(value: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", encoder.encode(value));
  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

export function randomToken(size = 32): string {
  const bytes = crypto.getRandomValues(new Uint8Array(size));
  return base64UrlEncode(bytes);
}

async function importHmacKey(secret: string): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign", "verify"],
  );
}

export async function signToken(
  payload: Record<string, unknown>,
  secret: string,
): Promise<string> {
  const header = base64UrlEncode(
    encoder.encode(JSON.stringify({ alg: "HS256", typ: "JWT" })),
  );
  const body = base64UrlEncode(encoder.encode(JSON.stringify(payload)));
  const signingInput = `${header}.${body}`;
  const key = await importHmacKey(secret);
  const signature = await crypto.subtle.sign("HMAC", key, encoder.encode(signingInput));
  return `${signingInput}.${base64UrlEncode(new Uint8Array(signature))}`;
}

export async function verifyToken<T>(
  token: string,
  secret: string,
): Promise<T | null> {
  const parts = token.split(".");
  if (parts.length !== 3) {
    return null;
  }

  const [header, body, signature] = parts;
  const key = await importHmacKey(secret);
  const valid = await crypto.subtle.verify(
    "HMAC",
    key,
    base64UrlDecode(signature),
    encoder.encode(`${header}.${body}`),
  );
  if (!valid) {
    return null;
  }

  try {
    const json = new TextDecoder().decode(base64UrlDecode(body));
    return JSON.parse(json) as T;
  } catch {
    return null;
  }
}

