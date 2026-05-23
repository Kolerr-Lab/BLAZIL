import type { Metadata } from "next";
import Link from "next/link";
import "./globals.css";

export const metadata: Metadata = {
  title: "Blazil Fintech Dashboard",
  description: "Real-time benchmark analytics and billing management for Blazil financial infrastructure",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body className="antialiased">
        <nav
          className="flex items-center gap-4 px-6 py-2 border-b border-gray-800 text-sm"
          style={{ background: "var(--bg)" }}
        >
          <Link href="/" className="text-gray-400 hover:text-white transition-colors">
            Benchmarks
          </Link>
          <Link href="/billing" className="text-gray-400 hover:text-white transition-colors">
            Billing
          </Link>
          <Link href="/billing/tenants" className="text-gray-400 hover:text-white transition-colors">
            Tenants
          </Link>
        </nav>
        {children}
      </body>
    </html>
  );
}
