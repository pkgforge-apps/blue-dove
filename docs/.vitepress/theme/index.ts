import type { Theme } from 'vitepress'
import DefaultTheme from 'vitepress/theme'

import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import './style.css'

import Downloads from './components/Downloads.vue'
import Home from './components/Home.vue'

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component('Downloads', Downloads)
    app.component('Home', Home)
  },
} satisfies Theme
