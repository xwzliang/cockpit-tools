import { invoke } from '@tauri-apps/api/core';

export type OpenAiUsageRange = 'today' | 'month' | '30d';

export interface OpenAiApiUsage {
  startTime: number;
  endTime: number;
  usage: {
    requests: number;
    inputTokens: number;
    outputTokens: number;
    cachedInputTokens: number;
  } | null;
  costUsd: number | null;
  usageError: string | null;
  costError: string | null;
}

export function queryOpenAiApiUsage(input: {
  adminKey: string;
  apiKeyId: string;
  projectId: string;
  range: OpenAiUsageRange;
}): Promise<OpenAiApiUsage> {
  return invoke('codex_query_openai_api_usage', {
    adminKey: input.adminKey.trim(),
    apiKeyId: input.apiKeyId.trim() || null,
    projectId: input.projectId.trim() || null,
    range: input.range,
  });
}
