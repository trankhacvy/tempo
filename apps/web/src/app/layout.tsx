import type { Metadata } from "next";
import { Geist, Geist_Mono, Nunito_Sans, Noto_Sans } from "next/font/google";

import { AppWalletProvider } from "@/components/wallet-provider";
import { Nav } from "@/components/layout/nav";

import "./globals.css";
import { cn } from "@/lib/utils";

const notoSansHeading = Noto_Sans({ subsets: ["latin"], variable: "--font-heading" });
const nunitoSans = Nunito_Sans({ subsets: ["latin"], variable: "--font-sans" });
const geistSans = Geist({ variable: "--font-geist-sans", subsets: ["latin"] });
const geistMono = Geist_Mono({ variable: "--font-geist-mono", subsets: ["latin"] });

export const metadata: Metadata = {
    title: "Tempo — Batch-Auction Perpetuals on Solana",
    description:
        "Tempo is a dual-flow batch-auction perpetuals DEX on Solana. Orders clear at one uniform price per auction — removing the speed advantage and MEV. Devnet client.",
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
    return (
        <html
            lang="en"
            className={cn(
                "dark",
                "h-full",
                "antialiased",
                geistSans.variable,
                geistMono.variable,
                "font-sans",
                nunitoSans.variable,
                notoSansHeading.variable,
            )}
        >
            <body className="flex h-full flex-col overflow-hidden font-sans">
                <AppWalletProvider>
                    <Nav />
                    <main id="top" className="min-h-0 flex-1 overflow-hidden">
                        {children}
                    </main>
                </AppWalletProvider>
            </body>
        </html>
    );
}
