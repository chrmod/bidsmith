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
            { label: 'Apply for Basic API access', slug: 'before-you-start/apply-for-basic-access' },
            { label: 'Connect to Google Ads', slug: 'before-you-start/connect-google-ads' },
            { label: 'Your first 10 minutes', slug: 'before-you-start/first-ten-minutes' },
          ],
        },
        {
          label: 'Tutorials',
          items: [
            { label: 'Launch a new search campaign', slug: 'tutorials/launch-search-campaign' },
            { label: 'Import an existing campaign', slug: 'tutorials/import-existing-campaign' },
          ],
        },
        {
          label: 'How-to recipes',
          collapsed: true,
          items: [
            { label: 'Overview', slug: 'recipes' },
            { label: 'Pause everything for the holidays', slug: 'recipes/pause-for-holidays' },
            { label: 'Move a keyword between match types', slug: 'recipes/change-keyword-match-type' },
            { label: 'Target specific countries and languages', slug: 'recipes/target-countries-and-languages' },
            { label: 'Add a whole keyword list at once', slug: 'recipes/add-many-keywords' },
            { label: 'Reuse a headline set across many ads', slug: 'recipes/reuse-a-headline-set' },
            { label: 'Reuse one ad across every ad group', slug: 'recipes/reuse-an-ad-across-ad-groups' },
            { label: 'Roll back a bad change', slug: 'recipes/roll-back-a-bad-change' },
            { label: 'Audit who changed what, when', slug: 'recipes/audit-who-changed-what' },
            { label: 'Cut Google Ads API quota usage', slug: 'recipes/reduce-api-calls' },
            { label: 'Manage multiple client accounts', slug: 'recipes/manage-multiple-accounts' },
            { label: 'See how your campaigns are performing', slug: 'recipes/see-campaign-performance' },
            { label: 'Organize a big account into folders', slug: 'recipes/organize-account-into-folders' },
            { label: 'Turn many cloned campaigns into one template', slug: 'recipes/collapse-cloned-campaigns' },
            { label: 'Rename a resource without recreating it', slug: 'recipes/rename-without-recreating' },
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
            { label: 'Locals', slug: 'concepts/locals' },
            { label: 'Variables', slug: 'concepts/variables' },
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
            { label: 'Overview', slug: 'commands' },
            {
              label: 'Local',
              items: [
                { label: 'bidsmith init', slug: 'commands/init' },
                { label: 'bidsmith validate', slug: 'commands/validate' },
                { label: 'bidsmith mv', slug: 'commands/mv' },
                { label: 'bidsmith fmt', slug: 'commands/fmt' },
                { label: 'bidsmith export', slug: 'commands/export' },
                { label: 'bidsmith schema', slug: 'commands/schema' },
                { label: 'bidsmith design-doc', slug: 'commands/design-doc' },
              ],
            },
            {
              label: 'API',
              items: [
                { label: 'bidsmith auth', slug: 'commands/auth' },
                { label: 'bidsmith plan', slug: 'commands/plan' },
                { label: 'bidsmith apply', slug: 'commands/apply' },
                { label: 'bidsmith refresh', slug: 'commands/refresh' },
                { label: 'bidsmith pull', slug: 'commands/pull' },
                { label: 'bidsmith query', slug: 'commands/query' },
              ],
            },
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
                { label: 'google_ads_shared_criterion', slug: 'resources/google_ads_shared_criterion' },
                { label: 'google_ads_campaign_shared_set', slug: 'resources/google_ads_campaign_shared_set' },
              ],
            },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Glossary', slug: 'reference/glossary' },
            { label: 'Privacy policy', slug: 'privacy' },
          ],
        },
      ],
    }),
  ],
});
