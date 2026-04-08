import { assertUserActive, getOrCreateUser, insertSession, revokeSession, rotateRefreshSession, type AuthenticatedUser } from "./db";
import { nowTs, randomToken, sha256Hex, signToken, verifyToken } from "./crypto";
import { HttpError } from "./http";

type AccessTokenPayload = {
  sub: string;
  email: string;
  sid: string;
  iat: number;
  exp: number;
};

function secretFromEnv(env: Env): string {
  const secret = (env as unknown as Record<string, unknown>).SYNC_SIGNING_KEY;
  if (typeof secret !== "string" || secret.length < 16) {
    throw new HttpError(500, "SYNC_SIGNING_KEY is not configured.");
  }
  return secret;
}

function ttlFromEnv(env: Env, name: keyof Pick<Env, "ACCESS_TOKEN_TTL_SECONDS" | "REFRESH_TOKEN_TTL_SECONDS">): number {
  const raw = env[name];
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new HttpError(500, `${name} must be a positive integer.`);
  }
  return parsed;
}

export async function issueSession(
  env: Env,
  user: AuthenticatedUser,
): Promise<{
  accessToken: string;
  refreshToken: string;
}> {
  const issuedAt = nowTs();
  const accessTtl = ttlFromEnv(env, "ACCESS_TOKEN_TTL_SECONDS");
  const refreshTtl = ttlFromEnv(env, "REFRESH_TOKEN_TTL_SECONDS");
  const sessionId = crypto.randomUUID();
  const refreshToken = randomToken(32);
  const refreshTokenHash = await sha256Hex(refreshToken);

  await insertSession(env.DB, {
    sessionId,
    userId: user.userId,
    refreshTokenHash,
    issuedAt,
    expiresAt: issuedAt + refreshTtl,
  });

  const accessToken = await signToken(
    {
      sub: user.userId,
      email: user.primaryEmail,
      sid: sessionId,
      iat: issuedAt,
      exp: issuedAt + accessTtl,
    } satisfies AccessTokenPayload,
    secretFromEnv(env),
  );

  return {
    accessToken,
    refreshToken,
  };
}

export async function rotateSession(
  env: Env,
  refreshToken: string,
): Promise<{
  user: AuthenticatedUser;
  accessToken: string;
  refreshToken: string;
}> {
  const issuedAt = nowTs();
  const refreshTtl = ttlFromEnv(env, "REFRESH_TOKEN_TTL_SECONDS");
  const refreshTokenHash = await sha256Hex(refreshToken);
  const replacement = randomToken(32);
  const replacementHash = await sha256Hex(replacement);
  const user = await rotateRefreshSession(
    env.DB,
    refreshTokenHash,
    replacementHash,
    issuedAt,
    issuedAt + refreshTtl,
  );

  const accessToken = await signToken(
    {
      sub: user.userId,
      email: user.primaryEmail,
      sid: crypto.randomUUID(),
      iat: issuedAt,
      exp: issuedAt + ttlFromEnv(env, "ACCESS_TOKEN_TTL_SECONDS"),
    } satisfies AccessTokenPayload,
    secretFromEnv(env),
  );

  return { user, accessToken, refreshToken: replacement };
}

export async function revokeRefreshToken(env: Env, refreshToken: string): Promise<void> {
  await revokeSession(env.DB, await sha256Hex(refreshToken), nowTs());
}

export async function authenticateRequest(
  request: Request,
  env: Env,
): Promise<AuthenticatedUser> {
  const authorization = request.headers.get("authorization");
  if (!authorization?.startsWith("Bearer ")) {
    throw new HttpError(401, "Missing bearer token.");
  }

  const token = authorization.slice("Bearer ".length).trim();
  const payload = await verifyToken<AccessTokenPayload>(token, secretFromEnv(env));
  if (!payload || payload.exp < nowTs()) {
    throw new HttpError(401, "Access token is invalid.");
  }

  return assertUserActive(env.DB, payload.sub);
}

export async function finishMagicLinkSignin(
  env: Env,
  email: string,
): Promise<{
  user: AuthenticatedUser;
  accessToken: string;
  refreshToken: string;
}> {
  const user = await getOrCreateUser(env.DB, email, nowTs());
  const session = await issueSession(env, user);
  return {
    user,
    accessToken: session.accessToken,
    refreshToken: session.refreshToken,
  };
}

