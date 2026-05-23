// Shared admin API client — server-side only.
// ADMIN_API_URL and ADMIN_TOKEN are never exposed to the browser.

const ADMIN_API_URL = process.env.ADMIN_API_URL ?? "http://localhost:8080";
const ADMIN_TOKEN = process.env.ADMIN_TOKEN ?? "";

function adminHeaders(): HeadersInit {
  return {
    "Content-Type": "application/json",
    Authorization: `Bearer ${ADMIN_TOKEN}`,
  };
}

export async function adminFetch(path: string, init?: RequestInit): Promise<Response> {
  return fetch(`${ADMIN_API_URL}${path}`, {
    ...init,
    headers: {
      ...adminHeaders(),
      ...(init?.headers ?? {}),
    },
    // Disable Next.js fetch cache for admin data — always fresh.
    cache: "no-store",
  });
}
