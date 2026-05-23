import { NextResponse } from "next/server";
import { adminFetch } from "@/lib/adminClient";

// GET /api/tenants  →  GET /v1/admin/tenants
export async function GET() {
  const res = await adminFetch("/v1/admin/tenants");
  const data = await res.json();
  return NextResponse.json(data, { status: res.status });
}
