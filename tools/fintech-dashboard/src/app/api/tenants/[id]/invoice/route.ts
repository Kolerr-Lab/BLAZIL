import { NextResponse } from "next/server";
import { adminFetch } from "@/lib/adminClient";

type Params = { params: Promise<{ id: string }> };

// GET /api/tenants/[id]/invoice  →  GET /v1/admin/tenants/{id}/invoice (live preview)
export async function GET(_req: Request, { params }: Params) {
  const { id } = await params;
  const res = await adminFetch(`/v1/admin/tenants/${id}/invoice`);
  const data = await res.json();
  return NextResponse.json(data, { status: res.status });
}
