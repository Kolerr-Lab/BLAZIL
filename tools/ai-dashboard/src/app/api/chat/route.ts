import { randomUUID } from "node:crypto";

import { NextRequest, NextResponse } from "next/server";

interface ChatProxyRequest {
  serverUrl: string;
  apiKey: string;
  tenantId: string;
  prompt: string;
  maxTokens?: number;
}

function normalizeServerUrl(raw: string): string | null {
  try {
    const url = new URL(raw);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return null;
    }
    return url.toString().replace(/\/$/, "");
  } catch {
    return null;
  }
}

export async function GET(request: NextRequest) {
  const serverUrl = normalizeServerUrl(request.nextUrl.searchParams.get("serverUrl") ?? "");
  if (!serverUrl) {
    return NextResponse.json({ error: "Invalid serverUrl" }, { status: 400 });
  }

  try {
    const response = await fetch(`${serverUrl}/health`, {
      method: "GET",
      cache: "no-store",
    });

    return NextResponse.json(
      {
        ok: response.ok,
        status: response.status,
      },
      { status: response.ok ? 200 : 502 },
    );
  } catch (error) {
    return NextResponse.json(
      {
        ok: false,
        error: error instanceof Error ? error.message : "Health check failed",
      },
      { status: 502 },
    );
  }
}

export async function POST(request: NextRequest) {
  let body: ChatProxyRequest;
  try {
    body = (await request.json()) as ChatProxyRequest;
  } catch {
    return NextResponse.json({ error: "Invalid JSON body" }, { status: 400 });
  }

  const serverUrl = normalizeServerUrl(body.serverUrl);
  if (!serverUrl) {
    return NextResponse.json({ error: "Invalid serverUrl" }, { status: 400 });
  }
  if (!body.apiKey.trim()) {
    return NextResponse.json({ error: "apiKey is required" }, { status: 400 });
  }
  if (!body.tenantId.trim()) {
    return NextResponse.json({ error: "tenantId is required" }, { status: 400 });
  }
  if (!body.prompt.trim()) {
    return NextResponse.json({ error: "prompt is required" }, { status: 400 });
  }

  try {
    const upstream = await fetch(`${serverUrl}/v1/chat`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        Authorization: `Bearer ${body.apiKey}`,
        "X-Tenant-ID": body.tenantId,
      },
      cache: "no-store",
      body: JSON.stringify({
        request_id: randomUUID(),
        prompt: body.prompt,
        max_tokens: body.maxTokens,
      }),
    });

    const responseText = await upstream.text();
    let payload: unknown;
    try {
      payload = JSON.parse(responseText);
    } catch {
      payload = { error: responseText || "Upstream returned non-JSON response" };
    }

    return NextResponse.json(payload, { status: upstream.status });
  } catch (error) {
    return NextResponse.json(
      {
        error: error instanceof Error ? error.message : "Chat request failed",
      },
      { status: 502 },
    );
  }
}