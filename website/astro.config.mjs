import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://chrmod.github.io',
  base: '/bidsmith',
  integrations: [
    starlight({
      title: 'bidsmith',
      description: 'Manage Google Ads campaigns as code. Version-controlled, reviewable, replayable.',
      social: {
        github: 'https://github.com/chrmod/bidsmith',
      },
      editLink: {
        baseUrl: 'https://github.com/chrmod/bidsmith/edit/main/website/',
      },
      lastUpdated: true,
      sidebar: [
        {
          label: 'Welcome',
          items: [
            { label: 'What is bidsmith?', slug: 'welcome/what-is-bidsmith' },
            { label: 'bidsmith vs Google Ads Editor', slug: 'welcome/vs-google-ads-editor' },
            { label: 'How the workflow looks', slug: 'welcome/workflow-overview' },
          ],
        },
        {
          label: 'Before you start',
          items: [
            { label: 'Install bidsmith', slug: 'before-you-start/install' },
            { label: 'Set up GitHub', slug: 'before-you-start/set-up-github' },
            { label: 'Connect to Google Ads', slug: 'before-you-start/connect-google-ads' },
            { label: 'Your first 10 minutes', slug: 'before-you-start/first-ten-minutes' },
          ],
        },
        {
          label: 'Tutorials',
          items: [
            { label: 'Launch a new search campaign', slug: 'tutorials/launch-search-campaign' },
          ],
        },
        {
          label: 'How-to recipes',
          collapsed: true,
          items: [
            { label: '(coming soon)', slug: 'recipes' },
          ],
        },
        {
          label: 'Core concepts',
          collapsed: true,
          items: [
            { label: '(coming soon)', slug: 'concepts' },
          ],
        },
        {
          label: 'Command reference',
          collapsed: true,
          items: [
            { label: '(coming soon)', slug: 'commands' },
          ],
        },
        {
          label: 'Resource reference',
          collapsed: true,
          items: [
            { label: '(coming soon)', slug: 'resources' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Glossary', slug: 'reference/glossary' },
          ],
        },
      ],
    }),
  ],
});
