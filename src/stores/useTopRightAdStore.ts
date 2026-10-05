import { create } from 'zustand';
import type { TopRightAd, TopRightAdState } from '../types/topRightAd';
import { forceRefreshTopRightAdState, getTopRightAdState } from '../services/topRightAdService';

const EMPTY_STATE: TopRightAdState = {
  ad: null,
  ads: [],
};

const TOP_RIGHT_AD_STATE_CACHE_KEY = 'agtools.top_right_ad_state.cache.v1';

interface TopRightAdStoreState {
  state: TopRightAdState;
  loading: boolean;
  initialized: boolean;
  fetchState: () => Promise<TopRightAdState>;
  forceRefreshState: () => Promise<TopRightAdState>;
}

function isTopRightAdState(value: unknown): value is TopRightAdState {
  if (!value || typeof value !== 'object') {
    return false;
  }
  const record = value as Partial<TopRightAdState>;
  return Array.isArray(record.ads);
}

function isBlockedTopRightAd(ad: TopRightAd | null | undefined): boolean {
  if (!ad) return false;
  return (
    ad.id === 'top-right-ad-2026-04-shop' ||
    (typeof ad.text === 'string' && ad.text.includes('GPT Plus 30天成品号')) ||
    (typeof ad.ctaUrl === 'string' && ad.ctaUrl.includes('xiangzili.xyz'))
  );
}

function normalizeTopRightAdState(state: TopRightAdState): TopRightAdState {
  const filteredAds = (Array.isArray(state.ads) ? state.ads : []).filter(
    (item) => !isBlockedTopRightAd(item),
  );
  const ad = !isBlockedTopRightAd(state.ad)
    ? (state.ad ?? filteredAds[0] ?? null)
    : (filteredAds[0] ?? null);
  return {
    ad,
    ads: filteredAds,
  };
}

function loadCachedTopRightAdState(): TopRightAdState {
  if (typeof localStorage === 'undefined') {
    return EMPTY_STATE;
  }

  try {
    const raw = localStorage.getItem(TOP_RIGHT_AD_STATE_CACHE_KEY);
    if (!raw) return EMPTY_STATE;
    const parsed = JSON.parse(raw) as { state?: unknown };
    return isTopRightAdState(parsed.state)
      ? normalizeTopRightAdState(parsed.state)
      : EMPTY_STATE;
  } catch {
    return EMPTY_STATE;
  }
}

function persistTopRightAdState(state: TopRightAdState): void {
  if (typeof localStorage === 'undefined') {
    return;
  }

  try {
    localStorage.setItem(
      TOP_RIGHT_AD_STATE_CACHE_KEY,
      JSON.stringify({ savedAt: Date.now(), state: normalizeTopRightAdState(state) }),
    );
  } catch {
    // 缓存写入失败不影响主流程。
  }
}

const initialTopRightAdState = loadCachedTopRightAdState();

export const useTopRightAdStore = create<TopRightAdStoreState>((set, get) => ({
  state: initialTopRightAdState,
  loading: false,
  initialized: initialTopRightAdState.ads.length > 0,

  fetchState: async () => {
    set({ loading: true });
    try {
      const nextState = normalizeTopRightAdState(await getTopRightAdState());
      set({ state: nextState, loading: false, initialized: true });
      persistTopRightAdState(nextState);
      return nextState;
    } catch (error) {
      console.error('加载右上角广告位失败:', error);
      const currentState = get().state;
      set({ state: currentState, loading: false, initialized: true });
      return currentState;
    }
  },

  forceRefreshState: async () => {
    set({ loading: true });
    try {
      const nextState = normalizeTopRightAdState(await forceRefreshTopRightAdState());
      set({ state: nextState, loading: false, initialized: true });
      persistTopRightAdState(nextState);
      return nextState;
    } catch (error) {
      console.error('强制刷新右上角广告位失败:', error);
      const currentState = get().state;
      set({ state: currentState, loading: false, initialized: true });
      return currentState;
    }
  },
}));
