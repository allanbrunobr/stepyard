import { useEffect, useRef } from 'react';

export function useAutoRefresh(callback: () => void, intervalMs: number = 30000) {
  const callbackRef = useRef(callback);
  callbackRef.current = callback;

  useEffect(() => {
    let timerId: ReturnType<typeof setInterval> | null = null;

    function start() {
      if (timerId) return;
      timerId = setInterval(() => callbackRef.current(), intervalMs);
    }

    function stop() {
      if (timerId) {
        clearInterval(timerId);
        timerId = null;
      }
    }

    function handleVisibilityChange() {
      if (document.hidden) {
        stop();
      } else {
        callbackRef.current();
        start();
      }
    }

    document.addEventListener('visibilitychange', handleVisibilityChange);
    if (!document.hidden) {
      start();
    }

    return () => {
      stop();
      document.removeEventListener('visibilitychange', handleVisibilityChange);
    };
  }, [intervalMs]);
}
