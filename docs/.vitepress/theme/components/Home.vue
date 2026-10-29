<script setup lang="ts">
import { useData, withBase } from 'vitepress'

import Downloads from './Downloads.vue'

interface Feature {
  title: string
  body: string
}

const { frontmatter, site } = useData()
const features: Feature[] = frontmatter.value.features ?? []
</script>

<template>
  <div class="home">
    <section class="hero">
      <img class="icon" :src="withBase('/logo.svg')" alt="" width="144" height="144" />
      <h1>{{ site.title }}</h1>
      <p class="tagline">{{ frontmatter.tagline }}</p>
      <p v-if="frontmatter.lead" class="lead">{{ frontmatter.lead }}</p>
      <div class="actions">
        <a class="button brand" :href="withBase('/guide/installation')">Download</a>
        <a class="button alt" :href="withBase('/guide/introduction')">Read the guide</a>
      </div>
    </section>

    <section v-if="frontmatter.screenshot" class="shot">
      <img :src="withBase(frontmatter.screenshot)" :alt="`${site.title} screenshot`" />
    </section>

    <section v-if="features.length" class="features">
      <article v-for="feature in features" :key="feature.title" class="card">
        <h3>{{ feature.title }}</h3>
        <p>{{ feature.body }}</p>
      </article>
    </section>

    <section class="get">
      <h2>Get it</h2>
      <Downloads />
    </section>
  </div>
</template>

<style scoped>
.home {
  max-width: 1080px;
  margin: 0 auto;
  padding: 72px 24px 96px;
}

.hero {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
}

.icon {
  filter: drop-shadow(0 18px 40px var(--app-glow));
  animation: float 6s ease-in-out infinite;
}

h1 {
  margin: 28px 0 8px;
  font-size: clamp(40px, 7vw, 64px);
  font-weight: 800;
  line-height: 1.05;
  letter-spacing: -0.03em;
  background: var(--app-gradient);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}

.tagline {
  margin: 0;
  font-size: clamp(20px, 3vw, 26px);
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.lead {
  max-width: 620px;
  margin: 16px 0 0;
  font-size: 17px;
  line-height: 1.6;
  color: var(--vp-c-text-2);
}

.actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 12px;
  margin-top: 32px;
}

.button {
  padding: 10px 22px;
  border-radius: 999px;
  font-weight: 700;
  text-decoration: none;
  transition: transform 0.15s, box-shadow 0.15s;
}

.button:hover {
  transform: translateY(-1px);
}

.button.brand {
  color: #fff;
  background: var(--app-gradient);
  box-shadow: 0 8px 24px var(--app-glow);
}

.button.alt {
  color: var(--vp-c-text-1);
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}

.shot {
  margin-top: 64px;
}

.shot img {
  width: 100%;
  border-radius: var(--app-radius);
  border: 1px solid var(--vp-c-divider);
  box-shadow: 0 30px 80px rgba(0, 0, 0, 0.18);
}

.features {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: 16px;
  margin-top: 72px;
}

.card {
  padding: 22px 24px;
  border-radius: var(--app-radius);
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}

.card h3 {
  margin: 0 0 8px;
  font-size: 17px;
  font-weight: 700;
}

.card p {
  margin: 0;
  line-height: 1.6;
  color: var(--vp-c-text-2);
}

.get {
  margin-top: 72px;
}

.get h2 {
  margin: 0;
  font-size: 28px;
  font-weight: 800;
  letter-spacing: -0.02em;
}

@keyframes float {
  50% {
    transform: translateY(-8px);
  }
}

@media (prefers-reduced-motion: reduce) {
  .icon {
    animation: none;
  }
}
</style>
