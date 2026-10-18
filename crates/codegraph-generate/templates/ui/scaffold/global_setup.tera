import { createClient } from '@supabase/supabase-js';
import type { FullConfig } from '@playwright/test';
import pg from 'pg';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const SUPABASE_URL = process.env.SUPABASE_URL ?? 'http://localhost:54321';
const SUPABASE_SERVICE_ROLE_KEY = process.env.SUPABASE_SERVICE_ROLE_KEY ?? '';
const SUPABASE_ANON_KEY = process.env.SUPABASE_ANON_KEY ?? '';
const DATABASE_URL = process.env.DATABASE_URL ?? 'postgres://postgres:postgres@localhost:54322/postgres';

const AUTH_DIR = path.resolve(__dirname);
const CONTEXT_FILE = path.join(AUTH_DIR, 'org-context.json');

// Initialized lazily inside globalSetup after env check
let supabaseAdmin: ReturnType<typeof createClient>;
let pgPool: pg.Pool;

async function createOrgUser(
  email: string,
  password: string,
  orgName: string,
): Promise<{ userId: string; orgId: string }> {
  const { data, error } = await supabaseAdmin.auth.admin.createUser({
    email,
    password,
    email_confirm: true,
    user_metadata: { org_name: orgName },
  });
  if (error) throw new Error(`Failed to create user ${email}: ${error.message}`);

  const userId = data.user.id;

  // The on_auth_user_created trigger creates a PERSONAL account (basejump
  // run_new_user_setup), but the generated UI gates owner actions on a
  // non-personal org (`find(a => !a.personal_account)`) — a personal-only
  // fixture left every owner-gated button unrendered (issue #468: 138
  // measured failures). Create a REAL team account and join the owner.
  const slug = orgName
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '')
    .slice(0, 40);
  const { rows } = await pgPool.query(
    `INSERT INTO basejump.accounts (primary_owner_user_id, name, slug, personal_account)
     VALUES ($1, $2, $3, false)
     RETURNING id`,
    [userId, orgName, slug],
  );
  const orgId = rows[0].id as string;
  await pgPool.query(
    'INSERT INTO basejump.account_user (account_id, user_id, account_role) VALUES ($1, $2, $3)',
    [orgId, userId, 'owner'],
  );

  // Remove the auto-created personal account so org resolution is unambiguous.
  await pgPool.query('DELETE FROM basejump.accounts WHERE primary_owner_user_id = $1 AND id != $2', [
    userId,
    orgId,
  ]);
  return { userId, orgId };
}

async function addUserToOrg(
  email: string,
  password: string,
  orgId: string,
  role: string,
): Promise<string> {
  const { data, error } = await supabaseAdmin.auth.admin.createUser({
    email,
    password,
    email_confirm: true,
  });
  if (error) throw new Error(`Failed to create user ${email}: ${error.message}`);

  const userId = data.user.id;

  await pgPool.query(
    'INSERT INTO basejump.account_user (account_id, user_id, account_role) VALUES ($1, $2, $3)',
    [orgId, userId, role],
  );

  // Delete the auto-created org (on_auth_user_created trigger creates one for every new user)
  await pgPool.query(
    'DELETE FROM basejump.accounts WHERE primary_owner_user_id = $1 AND id != $2',
    [userId, orgId],
  );

  return userId;
}

async function createApiKey(orgId: string, name: string): Promise<string> {
  const { rows } = await pgPool.query(
    `SELECT public.create_api_key($1::uuid, $2, '[{"entity_type":"*","entity_id":"*","action":"*"}]'::jsonb)`,
    [orgId, name],
  );
  const result = rows[0].create_api_key;
  const parsed = typeof result === 'string' ? JSON.parse(result) : result;
  return parsed.key;
}

function base64urlEncode(str: string): string {
  return Buffer.from(str, 'utf-8').toString('base64url');
}

async function saveStorageState(
  email: string,
  password: string,
  fileName: string,
): Promise<void> {
  // Sign in via Node.js Supabase client — no browser needed.
  // This avoids hydration timing issues and cookie race conditions
  // that made the previous browser-based login unreliable.
  const anonClient = createClient(SUPABASE_URL, SUPABASE_ANON_KEY, {
    auth: { autoRefreshToken: false, persistSession: false },
  });

  const { data, error } = await anonClient.auth.signInWithPassword({
    email,
    password,
  });

  if (error) {
    throw new Error(`Login failed for ${email}: ${error.message}`);
  }

  // Build the cookie value matching @supabase/ssr's cookieEncoding: "base64url"
  const sessionJson = JSON.stringify(data.session);
  const encoded = 'base64-' + base64urlEncode(sessionJson);

  // Derive cookie name: sb-<hostname-first-segment>-auth-token
  const ref = new URL(SUPABASE_URL).hostname.split('.')[0];
  const cookieName = `sb-${ref}-auth-token`;

  // 400-day maxAge matching @supabase/ssr DEFAULT_COOKIE_OPTIONS
  const maxAge = 400 * 24 * 60 * 60;
  const expires = Math.floor(Date.now() / 1000) + maxAge;

  // Write Playwright storage state JSON directly
  const storageState = {
    cookies: [
      {
        name: cookieName,
        value: encoded,
        domain: 'localhost',
        path: '/',
        expires,
        httpOnly: false,
        secure: false,
        sameSite: 'Lax' as const,
      },
    ],
    origins: [],
  };

  const statePath = path.join(AUTH_DIR, fileName);
  fs.writeFileSync(statePath, JSON.stringify(storageState, null, 2));
}

const timestamp = Date.now();

const ACME_OWNER_EMAIL = `test-acme-owner-${timestamp}@crewbase.test`;
const ACME_EMPLOYEE_EMAIL = `test-acme-employee-${timestamp}@crewbase.test`;
const ACME_MANAGER_EMAIL = `test-acme-manager-${timestamp}@crewbase.test`;
const HIGHFIVE_OWNER_EMAIL = `test-highfive-owner-${timestamp}@crewbase.test`;
const TEST_PASSWORD = 'TestPassword123!';

async function globalSetup(config: FullConfig) {
  if (!SUPABASE_SERVICE_ROLE_KEY) {
    console.log('[setup] SUPABASE_SERVICE_ROLE_KEY not set — skipping persona setup.');
    console.log('[setup] Only API-key tests will be available.');
    return;
  }

  supabaseAdmin = createClient(SUPABASE_URL, SUPABASE_SERVICE_ROLE_KEY, {
    auth: { autoRefreshToken: false, persistSession: false },
  });
  pgPool = new pg.Pool({ connectionString: DATABASE_URL });

  console.log('[setup] Creating ACME Corp org and owner...');
  const acme = await createOrgUser(ACME_OWNER_EMAIL, TEST_PASSWORD, `Test ACME ${timestamp}`);

  console.log('[setup] Creating HighFive Inc org and owner...');
  const highfive = await createOrgUser(HIGHFIVE_OWNER_EMAIL, TEST_PASSWORD, `Test HighFive ${timestamp}`);

  console.log('[setup] Adding ACME employee...');
  const employeeId = await addUserToOrg(ACME_EMPLOYEE_EMAIL, TEST_PASSWORD, acme.orgId, 'member');

  console.log('[setup] Adding ACME manager...');
  const managerId = await addUserToOrg(ACME_MANAGER_EMAIL, TEST_PASSWORD, acme.orgId, 'member');

  console.log('[setup] Creating ACME API key...');
  const acmeApiKey = await createApiKey(acme.orgId, `test-acme-key-${timestamp}`);

  console.log('[setup] Creating HighFive API key...');
  const highfiveApiKey = await createApiKey(highfive.orgId, `test-highfive-key-${timestamp}`);

  // Create per-worker UNIQUE USERS to avoid Supabase refresh-token invalidation.
  // Supabase invalidates all other refresh tokens for a user when any one token
  // is refreshed.  With multiple sessions for the same user, the first worker to
  // auto-refresh kills every other worker's session.  Creating dedicated users
  // per worker eliminates this entirely.
  const numWorkers = Number(process.env.PW_WORKER_COUNT ?? '8');
  console.log(`[setup] Creating per-worker users for ${numWorkers} workers...`);

  // Worker 0 reuses the original users (already created above)
  await saveStorageState(ACME_OWNER_EMAIL, TEST_PASSWORD, 'acme-owner-0.storageState.json');
  await saveStorageState(ACME_EMPLOYEE_EMAIL, TEST_PASSWORD, 'acme-employee-0.storageState.json');
  await saveStorageState(ACME_MANAGER_EMAIL, TEST_PASSWORD, 'acme-manager-0.storageState.json');
  await saveStorageState(HIGHFIVE_OWNER_EMAIL, TEST_PASSWORD, 'highfive-owner-0.storageState.json');

  // Workers 1..N each get their own unique user accounts
  for (let i = 1; i < numWorkers; i++) {
    const ownerEmail = `test-acme-owner-w${i}-${timestamp}@crewbase.test`;
    const employeeEmail = `test-acme-employee-w${i}-${timestamp}@crewbase.test`;
    const managerEmail = `test-acme-manager-w${i}-${timestamp}@crewbase.test`;
    const highfiveEmail = `test-highfive-owner-w${i}-${timestamp}@crewbase.test`;

    await addUserToOrg(ownerEmail, TEST_PASSWORD, acme.orgId, 'owner');
    await addUserToOrg(employeeEmail, TEST_PASSWORD, acme.orgId, 'member');
    await addUserToOrg(managerEmail, TEST_PASSWORD, acme.orgId, 'member');
    await addUserToOrg(highfiveEmail, TEST_PASSWORD, highfive.orgId, 'owner');

    await saveStorageState(ownerEmail, TEST_PASSWORD, `acme-owner-${i}.storageState.json`);
    await saveStorageState(employeeEmail, TEST_PASSWORD, `acme-employee-${i}.storageState.json`);
    await saveStorageState(managerEmail, TEST_PASSWORD, `acme-manager-${i}.storageState.json`);
    await saveStorageState(highfiveEmail, TEST_PASSWORD, `highfive-owner-${i}.storageState.json`);
  }

  // Save context for tests and teardown
  const context = {
    timestamp,
    acme: {
      orgId: acme.orgId,
      ownerId: acme.userId,
      employeeId,
      managerId,
      apiKey: acmeApiKey,
      ownerEmail: ACME_OWNER_EMAIL,
      employeeEmail: ACME_EMPLOYEE_EMAIL,
      managerEmail: ACME_MANAGER_EMAIL,
    },
    highfive: {
      orgId: highfive.orgId,
      ownerId: highfive.userId,
      ownerEmail: HIGHFIVE_OWNER_EMAIL,
      apiKey: highfiveApiKey,
    },
  };
  fs.writeFileSync(CONTEXT_FILE, JSON.stringify(context, null, 2));

  await pgPool.end();
  console.log('[setup] Done. ACME org:', acme.orgId, '| HighFive org:', highfive.orgId);
}

export default globalSetup;
