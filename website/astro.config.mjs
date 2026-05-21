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
            { label: 'Overview', slug: 'concepts' },
            { label: 'The .bid file', slug: 'concepts/bid-file' },
            { label: 'Plan and apply', slug: 'concepts/plan-and-apply' },
            { label: 'Modules', slug: 'concepts/modules' },
            { label: 'References', slug: 'concepts/references' },
            { label: 'Drift', slug: 'concepts/drift' },
            { label: 'The GitHub flow for marketers', slug: 'concepts/github-flow' },
            { label: 'Authentication', slug: 'concepts/authentication' },
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
            { label: 'Overview', slug: 'resources' },
            { label: 'provider "google_ads"', slug: 'resources/provider' },
            {
              label: 'Campaigns',
              items: [
                { label: 'google_ads_campaign', slug: 'resources/google_ads_campaign' },
                { label: 'google_ads_campaign_budget', slug: 'resources/google_ads_campaign_budget' },
              ],
            },
            {
              label: 'Ad groups and ads',
              items: [
                { label: 'google_ads_ad_group', slug: 'resources/google_ads_ad_group' },
                { label: 'google_ads_ad_group_ad', slug: 'resources/google_ads_ad_group_ad' },
              ],
            },
            {
              label: 'Keywords and targeting',
              items: [
                { label: 'google_ads_ad_group_criterion', slug: 'resources/google_ads_ad_group_criterion' },
                { label: 'google_ads_campaign_criterion', slug: 'resources/google_ads_campaign_criterion' },
              ],
            },
            {
              label: 'Conversion tracking',
              items: [
                { label: 'google_ads_conversion_action', slug: 'resources/google_ads_conversion_action' },
                { label: 'google_ads_call_asset', slug: 'resources/google_ads_call_asset' },
                { label: 'google_ads_customer_asset', slug: 'resources/google_ads_customer_asset' },
              ],
            },
            {
              label: 'Shared resources',
              items: [
                { label: 'google_ads_shared_set', slug: 'resources/google_ads_shared_set' },
                { label: 'google_ads_campaign_shared_set', slug: 'resources/google_ads_campaign_shared_set' },
              ],
            },
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
