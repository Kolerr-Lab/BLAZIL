import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Clarken Console | Blazil",
  description: "Clarken runtime console with the legacy Blazil benchmark workbench restored below",
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
