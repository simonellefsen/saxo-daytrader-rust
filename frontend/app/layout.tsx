import type { Metadata } from "next";

import "./globals.css";

export const metadata: Metadata = {
  title: "saxo-daytrader-xai",
  description: "Modern web frontend for the Saxo day trader runtime.",
  icons: {
    icon: "/icon.svg",
  },
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
