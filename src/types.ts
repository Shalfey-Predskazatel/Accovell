export type UsageWindow = {
  usedPercent: number;
  windowMinutes: number | null;
  resetsAt: number | null;
};

export type UsageSnapshot = {
  primary: UsageWindow | null;
  secondary: UsageWindow | null;
  updatedAt: number;
  error: string | null;
};

export type AccountStatusKind =
  | "healthy"
  | "usageLimit"
  | "reauthRequired"
  | "accessDenied"
  | "banned"
  | "corruptAuth"
  | "temporaryError";

export type AccountStatus = {
  kind: AccountStatusKind;
  detail: string | null;
};

export type Account = {
  id: string;
  accountId: string;
  email: string;
  plan: string;
  createdAt: number;
  lastUsedAt: number | null;
  usage: UsageSnapshot | null;
  status: AccountStatus;
  isActive: boolean;
};

export type AppSettings = {
  startWithWindows: boolean;
  startInTray: boolean;
  soundEnabled: boolean;
  switchThreshold: number;
  automaticProfileSwitching: boolean;
  refreshIntervalSeconds: number;
  proxyEnabled: boolean;
  theme: "system" | "dark" | "light";
  visualEffectsEnabled: boolean;
};

export type CodexPreferences = {
  model: string;
  modelReasoningEffort: string;
  approvalPolicy: string;
  sandboxMode: string;
  developerInstructions: string;
};

export type Prompt = { name: string; content: string };

export type DashboardData = {
  accounts: Account[];
  settings: AppSettings;
  currentAccountId: string | null;
  refreshedAt: number | null;
  codexPreferences: CodexPreferences;
  prompts: Prompt[];
  codexConnected: boolean;
  codexRunning: boolean;
  proxyAddress: string;
  proxyError: string | null;
  adminProtected: boolean;
  adminUnlocked: boolean;
};
