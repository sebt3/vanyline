import { getMe } from '$lib/api/auth';

let email = $state<string | null>(null);
let loading = $state(true);

export const userStore = {
  get email() { return email; },
  get loading() { return loading; },
  async load() {
    try {
      const me = await getMe();
      email = me.email;
    } catch {
      email = null;
    } finally {
      loading = false;
    }
  },
  clear() {
    email = null;
    loading = false;
  },
};
