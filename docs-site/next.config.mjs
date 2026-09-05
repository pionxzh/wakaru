import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  // Served at wakarujs.com/docs via the main site's proxy (see website/vercel.json).
  basePath: '/docs',
};

export default withMDX(config);
