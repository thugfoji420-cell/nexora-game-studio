import { useEffect, useRef, useCallback } from "react";

interface UseJobPollingOptions<T> {
  getJobDetails: (jobId: string) => Promise<T>;
  isActive: (job: T) => boolean;
  onUpdate: (job: T) => void;
  onError?: (error: unknown) => void;
  intervalMs?: number;
}

export function useJobPolling<T>({
  getJobDetails,
  isActive,
  onUpdate,
  onError,
  intervalMs = 1000,
}: UseJobPollingOptions<T>) {
  const activeRef = useRef(true);
  const inFlightRef = useRef(false);
  const jobIdRef = useRef<string | null>(null);

  const stop = useCallback(() => {
    activeRef.current = false;
  }, []);

  useEffect(() => {
    return () => {
      activeRef.current = false;
    };
  }, []);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;

    const poll = async () => {
      const jobId = jobIdRef.current;
      if (!jobId || !activeRef.current || inFlightRef.current) return;

      inFlightRef.current = true;
      try {
        const next = await getJobDetails(jobId);
        if (activeRef.current && jobIdRef.current === jobId) {
          onUpdate(next);
        }
      } catch (nextError) {
        if (activeRef.current && onError) {
          onError(nextError);
        }
      } finally {
        inFlightRef.current = false;
        if (activeRef.current) {
          timer = setTimeout(poll, intervalMs);
        }
      }
    };

    const start = async () => {
      await poll();
      if (activeRef.current) {
        timer = setTimeout(poll, intervalMs);
      }
    };

    start();

    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [getJobDetails, onUpdate, onError, intervalMs]);

  return {
    startPolling: (jobId: string) => {
      jobIdRef.current = jobId;
    },
    stopPolling: stop,
  };
}
