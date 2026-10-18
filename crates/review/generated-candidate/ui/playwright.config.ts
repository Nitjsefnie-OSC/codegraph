import { defineConfig } from '@playwright/test';
import dotenv from 'dotenv';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
dotenv.config({ path: path.resolve(__dirname, '.env.test') });

export default defineConfig({
  testDir: './tests',
  timeout: 60_000,
  workers: Number(process.env.PW_WORKER_COUNT ?? '4'),
  use: {
    baseURL: process.env.PUBLIC_SVELTEKIT_URL ?? 'http://localhost:5173',
    trace: 'on-first-retry',
  },
  globalSetup: './tests/e2e/auth/global-setup.ts',
  globalTeardown: './tests/e2e/auth/global-teardown.ts',
  projects: [
    {
      name: 'journey',
      testMatch: /e2e\/journey\/.+\.test\.ts/,
    },
    {
      name: 'crud',
      testMatch: /generated\/\w+\/[\w-]+\.(?!api\.)([\w-]+\.)?(crud|view|team|isolation|validation|workflow)\.test\.ts/,
      fullyParallel: true,
    },
    {
      name: 'e2e',
      testMatch: /e2e\/(?!auth|fixtures|security|journey|webhooks)[\w-]+\/.+\.test\.ts/,
      fullyParallel: true,
    },
    {
      name: 'security',
      testMatch: /e2e\/security\/.+\.test\.ts/,
      fullyParallel: true,
    },
    {
      name: 'api',
      testMatch: /generated\/\w+\/[\w-]+\.api\.crud\.test\.ts/,
      fullyParallel: true,
    },
    {
      name: 'webhooks',
      testMatch: /e2e\/webhooks\/.+\.test\.ts/,
      fullyParallel: true,
    },
    {
      // IFML modeled views (issue #468 W1b): specs, journeys, gating, and
      // auth suites live under tests/ifml/ and end .spec.ts — disjoint
      // from the entity projects' .test.ts globs.
      name: 'ifml',
      testMatch: /ifml\/.+\.spec\.ts/,
    },
  ],
});
