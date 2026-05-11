import { headers } from "next/headers";
import { NextResponse } from "next/server";

function firstHeaderValue(value: string | null): string | null {
  if (!value) {
    return null;
  }
  return value.split(",")[0]?.trim() || null;
}

export async function GET() {
  const requestHeaders = await headers();
  const email = firstHeaderValue(requestHeaders.get("x-daytrader-user-email"));
  const name = firstHeaderValue(requestHeaders.get("x-daytrader-user-name"));

  return NextResponse.json({
    authenticated: Boolean(email),
    user: email
      ? {
          email,
          name: name || email,
        }
      : null,
  });
}
