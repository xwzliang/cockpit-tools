import { useId, useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ExternalLink, RefreshCw } from 'lucide-react';
import {
  queryOpenAiApiUsage,
  type OpenAiApiUsage,
  type OpenAiUsageRange,
} from '../../services/openaiApiUsageService';
import '../../styles/pages/openai-api-usage.css';

export function OpenAiApiUsagePanel() {
  const { t, i18n } = useTranslation();
  const id = useId();
  const [adminKey, setAdminKey] = useState('');
  const [apiKeyId, setApiKeyId] = useState('');
  const [projectId, setProjectId] = useState('');
  const [range, setRange] = useState<OpenAiUsageRange>('month');
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<OpenAiApiUsage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const number = new Intl.NumberFormat(i18n.language);
  const money = new Intl.NumberFormat(i18n.language, {
    style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: 4,
  });
  const date = new Intl.DateTimeFormat(i18n.language, {
    month: 'short', day: 'numeric', year: 'numeric', timeZone: 'UTC',
  });

  function resetResult() {
    setResult(null);
    setError(null);
  }

  function errorText(code: string) {
    if (code.includes('ADMIN_KEY_REQUIRED')) {
      return t('openaiApiUsage.errors.adminKey', 'An organization Admin API key with usage reporting access is required. Regular API keys cannot read these reports.');
    }
    if (code.includes('KEY_ID_REQUIRED')) {
      return t('openaiApiUsage.errors.keyId', 'Enter the API key ID (key_…), not its secret (sk-…).');
    }
    if (code.includes('RATE_LIMITED')) {
      return t('openaiApiUsage.errors.rateLimit', 'OpenAI rate limited this request. Try again later.');
    }
    if (code.includes('NETWORK_FAILED')) {
      return t('openaiApiUsage.errors.network', 'Could not connect to OpenAI. Check your connection and try again.');
    }
    return t('openaiApiUsage.errors.query', 'Could not load this report. Check the Admin key permissions and filter IDs, then try again.');
  }

  async function refresh(event: FormEvent) {
    event.preventDefault();
    if (loading) return;
    resetResult();
    setLoading(true);
    try {
      setResult(await queryOpenAiApiUsage({ adminKey, apiKeyId, projectId, range }));
    } catch (cause) {
      setError(errorText(String(cause)));
    } finally {
      setLoading(false);
    }
  }

  async function openPlatform(url: string) {
    try {
      await openUrl(url);
    } catch {
      setError(t('openaiApiUsage.errors.browser', 'Could not open OpenAI in your browser.'));
    }
  }

  const metrics = [
    [t('openaiApiUsage.cost', 'API cost'), result?.costUsd == null ? '—' : money.format(result.costUsd)],
    [t('openaiApiUsage.requests', 'Model requests'), result?.usage ? number.format(result.usage.requests) : '—'],
    [t('openaiApiUsage.inputTokens', 'Input tokens'), result?.usage ? number.format(result.usage.inputTokens) : '—'],
    [t('openaiApiUsage.outputTokens', 'Output tokens'), result?.usage ? number.format(result.usage.outputTokens) : '—'],
    [t('openaiApiUsage.cachedTokens', 'Cached input tokens'), result?.usage ? number.format(result.usage.cachedInputTokens) : '—'],
  ];

  return (
    <section className="openai-api-usage" aria-labelledby={`${id}-title`} aria-busy={loading}>
      <div className="openai-api-usage-heading">
        <div>
          <h2 id={`${id}-title`}>{t('openaiApiUsage.title', 'OpenAI API usage')}</h2>
          <p>{t('openaiApiUsage.description', 'View API usage separately from your Codex subscription limits. An organization Admin API key is required.')}</p>
        </div>
        <button type="button" className="btn btn-secondary" onClick={() => void openPlatform('https://platform.openai.com/settings/organization/admin-keys')}>
          {t('openaiApiUsage.adminKeys', 'Admin API keys')} <ExternalLink size={14} />
        </button>
      </div>
      <form onSubmit={refresh}>
        <fieldset disabled={loading} className="openai-api-usage-form">
          <label htmlFor={`${id}-key`}>
            {t('openaiApiUsage.adminKey', 'Admin API key')}
            <input id={`${id}-key`} type="password" autoComplete="off" spellCheck={false} required
              value={adminKey} onChange={(event) => { setAdminKey(event.target.value); resetResult(); }} />
          </label>
          <label htmlFor={`${id}-filter`}>
            {t('openaiApiUsage.apiKeyId', 'API key ID (optional)')}
            <input id={`${id}-filter`} type="text" autoComplete="off" spellCheck={false} placeholder="key_…"
              value={apiKeyId} onChange={(event) => { setApiKeyId(event.target.value); resetResult(); }} />
          </label>
          <label htmlFor={`${id}-project`}>
            {t('openaiApiUsage.projectId', 'Project ID (optional)')}
            <input id={`${id}-project`} type="text" autoComplete="off" spellCheck={false} placeholder="proj_…"
              value={projectId} onChange={(event) => { setProjectId(event.target.value); resetResult(); }} />
          </label>
          <label htmlFor={`${id}-range`}>
            {t('openaiApiUsage.period', 'Period (UTC)')}
            <select id={`${id}-range`} value={range} onChange={(event) => { setRange(event.target.value as OpenAiUsageRange); resetResult(); }}>
              <option value="today">{t('openaiApiUsage.today', 'Today')}</option>
              <option value="month">{t('openaiApiUsage.month', 'Month to date')}</option>
              <option value="30d">{t('openaiApiUsage.thirtyDays', 'Last 30 days')}</option>
            </select>
          </label>
          <button type="submit" className="btn btn-primary" disabled={loading || !adminKey.trim()}>
            <RefreshCw size={14} className={loading ? 'spin' : undefined} />
            {loading ? t('openaiApiUsage.loading', 'Loading…') : t('openaiApiUsage.refresh', 'Refresh usage')}
          </button>
          <button type="button" className="btn btn-secondary" disabled={!adminKey && !result && !error}
            onClick={() => { setAdminKey(''); resetResult(); }}>
            {t('openaiApiUsage.clear', 'Clear key')}
          </button>
        </fieldset>
      </form>
      <p className="openai-api-usage-hint">{t('openaiApiUsage.keyHint', 'The Admin key stays in memory until you leave this view. Leave both filters empty for organization totals. To view one key, enter its ID from the project API keys page.')}</p>
      <div aria-live="polite">
        {error && <p className="openai-api-usage-error" role="alert">{error}</p>}
        {result && <>
          <p className="openai-api-usage-scope">
            {apiKeyId.trim() ? t('openaiApiUsage.keyScope', 'API key: {{id}}', { id: apiKeyId.trim() }) : t('openaiApiUsage.organizationScope', 'All API keys')}
            {' · '}{projectId.trim() ? t('openaiApiUsage.projectScope', 'Project: {{id}}', { id: projectId.trim() }) : t('openaiApiUsage.allProjects', 'All projects')}
            {' · '}{date.format(result.startTime * 1000)} – {date.format(result.endTime * 1000)} (UTC)
          </p>
          <dl className="openai-api-usage-metrics">
            {metrics.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
          </dl>
          {result.usageError && <p className="openai-api-usage-error" role="alert">{t('openaiApiUsage.usageFailed', 'Usage report: {{error}}', { error: errorText(result.usageError) })}</p>}
          {result.costError && <p className="openai-api-usage-error" role="alert">{t('openaiApiUsage.costFailed', 'Cost report: {{error}}', { error: errorText(result.costError) })}</p>}
          <p className="openai-api-usage-hint">{t('openaiApiUsage.reportingHint', 'Requests and tokens cover the completions usage report. Cached tokens are included in input tokens. Costs cover all API products in the selected scope. OpenAI reporting may be delayed.')}</p>
        </>}
      </div>
      <div className="openai-api-usage-billing">
        <p>{t('openaiApiUsage.balanceHint', 'Credit balance is not available through a documented OpenAI API. View your remaining credits in OpenAI billing.')}</p>
        <button type="button" className="btn btn-secondary" onClick={() => void openPlatform('https://platform.openai.com/settings/organization/billing/overview')}>
          {t('openaiApiUsage.billing', 'View credit balance')} <ExternalLink size={14} />
        </button>
      </div>
    </section>
  );
}
