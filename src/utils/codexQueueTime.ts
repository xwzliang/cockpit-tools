const pad = (value: number) => String(value).padStart(2, '0');

/** Local wall-clock fields, without locale-dependent AM/PM controls. */
export function toLocalQueueDateTime(timestampSeconds?: number): { date: string; time: string } {
  if (timestampSeconds == null) return { date: '', time: '' };
  const date = new Date(timestampSeconds * 1000);
  if (!Number.isFinite(date.getTime())) return { date: '', time: '' };
  return {
    date: `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
    time: `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`,
  };
}

export function formatQueueEntryTime24h(timestampSeconds: number): string {
  const { date, time } = toLocalQueueDateTime(timestampSeconds);
  return `${date} ${time}`.trim();
}

export function formatQueueTimeRemaining(timestampSeconds: number, nowMs = Date.now()): string {
  const seconds = Math.max(0, Math.ceil((timestampSeconds * 1000 - nowMs) / 1000));
  const days = Math.floor(seconds / 86400);
  const clock = `${pad(Math.floor(seconds / 3600) % 24)}:${pad(Math.floor(seconds / 60) % 60)}:${pad(seconds % 60)}`;
  return days > 0 ? `${days}d ${clock}` : clock;
}

/** Empty fields mean immediate eligibility. Reject invalid dates and DST gaps. */
export function parseLocalQueueDateTime(date: string, time: string): number | undefined {
  if (!date && !time) return undefined;
  const dateMatch = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  const timeMatch = /^(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(time);
  if (!dateMatch || !timeMatch) throw new Error('Invalid local date or 24-hour time');
  const [, year, month, day] = dateMatch.map(Number);
  const hour = Number(timeMatch[1]);
  const minute = Number(timeMatch[2]);
  const second = Number(timeMatch[3] || 0);
  const value = new Date(0);
  value.setFullYear(year, month - 1, day);
  value.setHours(hour, minute, second, 0);
  if (year < 1 || value.getFullYear() !== year || value.getMonth() !== month - 1 ||
      value.getDate() !== day || value.getHours() !== hour || value.getMinutes() !== minute ||
      value.getSeconds() !== second || !Number.isFinite(value.getTime())) {
    throw new Error('Invalid local date or 24-hour time');
  }
  return Math.floor(value.getTime() / 1000);
}
