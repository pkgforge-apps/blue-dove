import { defineConfig } from 'vitepress'

import { app } from './app'

const { name, slug, description, repo, accent } = app

export default defineConfig({
  title: name,
  description,
  base: `/${slug}/`,
  lang: 'en-US',
  cleanUrls: true,
  lastUpdated: true,

  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: `/${slug}/favicon.svg` }],
    ['link', { rel: 'apple-touch-icon', href: `/${slug}/apple-touch-icon.png` }],
    ['meta', { name: 'theme-color', content: accent }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:title', content: name }],
    ['meta', { property: 'og:description', content: description }],
  ],

  themeConfig: {
    logo: '/logo.svg',

    nav: [
      { text: 'Guide', link: '/guide/introduction', activeMatch: '/guide/' },
      { text: 'Download', link: '/guide/installation' },
      { text: 'Releases', link: `${repo}/releases` },
    ],

    sidebar: [
      {
        text: 'Guide',
        items: [
          { text: 'Introduction', link: '/guide/introduction' },
          { text: 'Installation', link: '/guide/installation' },
          { text: 'Using Blue Dove', link: '/guide/using' },
        ],
      },
    ],

    socialLinks: [{ icon: 'github', link: repo }],

    editLink: {
      pattern: `${repo}/edit/main/docs/:path`,
      text: 'Edit this page',
    },

    search: { provider: 'local' },

    outline: { level: [2, 3], label: 'On this page' },

    footer: {
      message: 'Released under the MIT license. Part of <a href="https://github.com/pkgforge-apps">pkgforge-apps</a>.',
    },
  },
})
