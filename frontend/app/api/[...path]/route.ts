import { NextRequest } from "next/server";

export const dynamic = "force-dynamic";
export const runtime = "nodejs";

type RouteContext = {
  params: Promise<{
    path?: string[];
  }>;
};

const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "content-encoding",
  "content-length",
  "host",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

function backendBaseUrl(): string {
  return (process.env.API_BASE_URL || "http://127.0.0.1:8000").replace(/\/+$/, "");
}

async function proxy(request: NextRequest, context: RouteContext): Promise<Response> {
  const params = await context.params;
  const path = (params.path || []).map(encodeURIComponent).join("/");
  const sourceUrl = new URL(request.url);
  const targetUrl = `${backendBaseUrl()}/api/${path}${sourceUrl.search}`;
  const headers = new Headers(request.headers);
  for (const header of HOP_BY_HOP_HEADERS) {
    headers.delete(header);
  }

  let response: Response;
  try {
    response = await fetch(targetUrl, {
      method: request.method,
      headers,
      body: request.method === "GET" || request.method === "HEAD" ? undefined : await request.arrayBuffer(),
      cache: "no-store",
      redirect: "manual",
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : "Backend request failed.";
    return Response.json(
      {
        detail: "Backend is temporarily unavailable. The frontend will keep retrying.",
        error: message,
      },
      {
        status: 503,
        headers: {
          "Retry-After": "5",
        },
      },
    );
  }
  const responseHeaders = new Headers(response.headers);
  for (const header of HOP_BY_HOP_HEADERS) {
    responseHeaders.delete(header);
  }
  if (!response.ok) {
    console.warn("Backend API proxy returned non-OK response", {
      method: request.method,
      path: `/api/${path}`,
      status: response.status,
      statusText: response.statusText,
    });
  }
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers: responseHeaders,
  });
}

export const GET = proxy;
export const POST = proxy;
export const PUT = proxy;
export const PATCH = proxy;
export const DELETE = proxy;
