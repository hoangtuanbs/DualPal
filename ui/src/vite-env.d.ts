/// <reference types="vite/client" />

interface Window {
  /** Set by the app in test mode; resolves after both panes finish their
   *  first listing (PREP-06). */
  __DUALPAL_TEST__?: {
    ready: Promise<void>;
  };
}
