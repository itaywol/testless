import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';

export default defineConfig({
  site: 'https://testless.itaywol.tools',
  integrations: [sitemap()],
});
