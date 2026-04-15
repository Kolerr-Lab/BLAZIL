import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Blazil Bench Dashboard",
  description: "Real-time benchmark analytics for Blazil financial infrastructure",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body className="antialiased">{children}</body>
    </html>
  );
}
