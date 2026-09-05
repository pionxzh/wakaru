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
      <head>
        {/* Vercel Web Analytics, same tag as the landing page. The path is
            deliberately not under the basePath: through the wakarujs.com/docs
            proxy it reaches the main site's project, so docs traffic lands in
            the same dashboard. */}
        <script defer src="/_vercel/insights/script.js" />
      </head>
      <body className="flex flex-col min-h-screen">
        <RootProvider
          theme={{ defaultTheme: 'dark' }}
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
