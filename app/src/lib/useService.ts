import { useCallback, useEffect, useRef, useState } from "react";
import { api, Loader, Model, Status, Unreachable } from "./api";

export interface Service {
  status: Status | null;
  models: Model[];
  loaders: Loader[];
  /** The service is not answering. */
  down: boolean;
  /** Fetch everything again now (after an action that changed something). */
  refresh: () => Promise<void>;
}

/** Keeps the service's state on screen: status every 1.5 s, the model and loader lists every 6 s. */
export function useService(): Service {
  const [status, setStatus] = useState<Status | null>(null);
  const [models, setModels] = useState<Model[]>([]);
  const [loaders, setLoaders] = useState<Loader[]>([]);
  const [down, setDown] = useState(false);
  const alive = useRef(true);

  const pullStatus = useCallback(async () => {
    try {
      const s = await api.status();
      if (alive.current) {
        setStatus(s);
        setDown(false);
      }
    } catch (e) {
      if (alive.current && e instanceof Unreachable) setDown(true);
    }
  }, []);

  const pullLists = useCallback(async () => {
    try {
      const [m, l] = await Promise.all([api.models(), api.loaders()]);
      if (alive.current) {
        setModels(m.models);
        setLoaders(l.loaders);
      }
    } catch {
      // the status poll reports a service that is down
    }
  }, []);

  const refresh = useCallback(async () => {
    await Promise.all([pullStatus(), pullLists()]);
  }, [pullStatus, pullLists]);

  useEffect(() => {
    alive.current = true;
    refresh();
    const fast = setInterval(pullStatus, 1500);
    const slow = setInterval(pullLists, 6000);
    return () => {
      alive.current = false;
      clearInterval(fast);
      clearInterval(slow);
    };
  }, [refresh, pullStatus, pullLists]);

  return { status, models, loaders, down, refresh };
}
