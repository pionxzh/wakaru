import { RootProvider } from 'fumadocs-ui/provider/next';
import './global.css';
import { Inter } from 'next/font/google';
import type { Metadata } from 'next';

const inter = Inter({
  subsets: ['latin'],
});

export const metadata: Metadata = {
  metadataBase: new URL('https://wakarujs.com'),
  title: {
    default: 'Wakaru Docs',
    template: '%s — Wakaru Docs',
  },
  description:
    'Documentation for Wakaru, the JavaScript decompiler and bundle unpacker.',
};

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html lang="en" className={inter.className} suppressHydrationWarning>
      <body className="flex flex-col min-h-screen">
        <RootProvider
          search={{
            // fetch() does not apply basePath automatically.
            options: { api: '/docs/api/search' },
          }}
        >
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
