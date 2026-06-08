import type { StorybookConfig } from '@storybook/svelte-vite';

const port = process.env.STORYBOOK_PORT ?? '6006';
const base = process.env.VSCODE_PROXY_URI ? `/proxy/${port}/` : '/';

const config: StorybookConfig = {
  stories: ['../src/**/*.stories.svelte'],
  addons: [{ name: '@storybook/addon-svelte-csf', options: { legacyTemplate: true } }],
  framework: '@storybook/svelte-vite',
  core: { allowedHosts: true },
  viteFinal: async (cfg) => ({
    ...cfg,
    base,
    plugins: [
      ...(cfg.plugins ?? []),
      {
        name: 'storybook-fix-proxy-base',
        enforce: 'post' as const,
        transformIndexHtml: {
          order: 'post' as const,
          handler(html: string) {
            if (base === '/') return html;
            return html.replace(/(src|href)="(\/[^"]+)"/g, (_match, attr, path) => {
              if (path.startsWith(base)) return `${attr}="${path}"`;
              return `${attr}="${base}${path.slice(1)}"`;
            });
          },
        },
      },
    ],
  }),
};

export default config;
