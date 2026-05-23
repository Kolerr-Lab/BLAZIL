import { NextResponse } from "next/server";
import { adminFetch } from "@/lib/adminClient";

type Params = { params: Promise<{ id: string }> };

// GET  /api/tenants/[id]/invoices  →  GET  /v1/admin/tenants/{id}/invoices (persisted list)
// POST /api/tenants/[id]/invoices  →  POST /v1/admin/tenants/{id}/invoices (generate draft)
export async function GET(_req: Request, { params }: Params) {
  const { id } = await params;
  const res = await adminFetch(`/v1/admin/tenants/${id}/invoices`);
  const data = await res.json();
  return NextResponse.json(data, { status: res.status });
}

export async function POST(_req: Request, { params }: Params) {
  const { id } = await params;
  const res = await adminFetch(`/v1/admin/tenants/${id}/invoices`, { method: "POST" });
  const data = await res.json();
  return NextResponse.json(data, { status: res.status });
}
