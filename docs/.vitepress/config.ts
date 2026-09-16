import { defineConfig } from 'vitepress'

const hostname = 'https://stagelint.dev'

export default defineConfig({
  title: 'stagelint',
  description:
    'Run linters and formatters on staged git files. A single binary with no runtime that never aborts your commit over a conflicting unstaged change.',
  cleanUrls: true,
  sitemap: { hostname },
  head: [
    ['link', { rel: 'icon', href: '/logo.svg' }],
    ['style', {}, '.VPFooter.has-sidebar{display:block!important}'],
  ],
  markdown: {
    config(md) {
      const codeInline = md.renderer.rules.code_inline!
      md.renderer.rules.code_inline = (...args) =>
        codeInline(...args).replace('<code', '<code v-pre')
    },
  },
  transformPageData(pageData, { siteConfig: { site } }) {
    const url = new URL(pageData.relativePath.replace(/(?:(^|\/)index)?\.md$/, '$1'), hostname).href
    const title = pageData.title ? `${pageData.title} | ${site.title}` : site.title
    const description = pageData.description || site.description

    pageData.frontmatter.head ??= []
    pageData.frontmatter.head.push(
      ['meta', { property: 'og:url', content: url }],
      ['meta', { property: 'og:title', content: title }],
      ['meta', { property: 'og:description', content: description }],
      ['meta', { property: 'og:type', content: 'website' }],
      ['meta', { property: 'og:site_name', content: site.title }],
      ['link', { rel: 'canonical', href: url }],
    )
  },
  themeConfig: {
    logo: '/logo.svg',
    search: { provider: 'local' },
    socialLinks: [
      { icon: 'github', link: 'https://github.com/abemedia/stagelint' },
    ],
    nav: [
      {
        text: 'Guide',
        link: '/introduction',
        activeMatch: '^/(introduction|installation|getting-started|configuration|troubleshooting)',
      },
      { text: 'Reference', link: '/cli', activeMatch: '^/cli' },
      {
        text: 'Integrations',
        link: '/integrations/',
        activeMatch: '^/(integrations|agent-hooks|ci|hook-managers)/',
      },
    ],
    sidebar: [
      {
        text: 'Guide',
        items: [
          { text: 'Introduction', link: '/introduction' },
          { text: 'Installation', link: '/installation' },
          { text: 'Getting started', link: '/getting-started' },
          { text: 'Configuration', link: '/configuration' },
          { text: 'Troubleshooting', link: '/troubleshooting' },
        ],
      },
      {
        text: 'Reference',
        items: [{ text: 'CLI', link: '/cli' }],
      },
      {
        text: 'Integrations',
        items: [
          {
            text: 'Continuous integration',
            link: '/ci/',
            collapsed: false,
            items: [
              { text: 'GitHub Actions', link: '/ci/github-actions' },
              { text: 'GitLab CI/CD', link: '/ci/gitlab' },
              { text: 'Other CI systems', link: '/ci/other' },
            ],
          },
          {
            text: 'Agent hooks',
            link: '/agent-hooks/',
            collapsed: false,
            items: [
              { text: 'Claude Code', link: '/agent-hooks/claude-code' },
              { text: 'Codex', link: '/agent-hooks/codex' },
              { text: 'GitHub Copilot CLI', link: '/agent-hooks/github-copilot-cli' },
              { text: 'Gemini CLI', link: '/agent-hooks/gemini-cli' },
              { text: 'Cursor', link: '/agent-hooks/cursor' },
              { text: 'aider', link: '/agent-hooks/aider' },
              { text: 'OpenCode', link: '/agent-hooks/opencode' },
            ],
          },
          {
            text: 'Hook managers',
            link: '/hook-managers/',
            collapsed: false,
            items: [
              { text: 'pre-commit', link: '/hook-managers/pre-commit' },
              { text: 'Lefthook', link: '/hook-managers/lefthook' },
              { text: 'husky', link: '/hook-managers/husky' },
            ],
          },
        ],
      },
    ],
    footer: { copyright: '&copy; 2026 Adam Bouqdib' },
    editLink: {
      pattern: 'https://github.com/abemedia/stagelint/edit/master/docs/:path',
    },
  },
})
