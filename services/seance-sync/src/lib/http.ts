export class HttpError extends Error {
  readonly status: number;
  readonly details?: unknown;

  constructor(status: number, message: string, details?: unknown) {
    super(message);
    this.status = status;
    this.details = details;
  }
}

export function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload, null, 2), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
    },
  });
}

export function emptyResponse(status = 204): Response {
  return new Response(null, { status });
}

export function textResponse(payload: string, status = 200): Response {
  return new Response(payload, {
    status,
    headers: {
      "content-type": "text/plain; charset=utf-8",
      "cache-control": "no-store",
    },
  });
}

export async function readJson<T>(request: Request): Promise<T> {
  try {
    return (await request.json()) as T;
  } catch {
    throw new HttpError(400, "Request body must be valid JSON.");
  }
}

export function requireMethod(request: Request, expected: string): void {
  if (request.method !== expected) {
    throw new HttpError(405, `Method ${request.method} is not allowed for this endpoint.`);
  }
}

export async function withErrorBoundary(
  handler: () => Promise<Response>,
): Promise<Response> {
  try {
    return await handler();
  } catch (error) {
    if (error instanceof HttpError) {
      return jsonResponse(
        {
          error: error.message,
          details: error.details ?? null,
        },
        error.status,
      );
    }

    console.error(
      JSON.stringify({
        level: "error",
        msg: "unhandled_request_error",
        error: error instanceof Error ? error.message : String(error),
      }),
    );
    return jsonResponse({ error: "Internal server error." }, 500);
  }
}

