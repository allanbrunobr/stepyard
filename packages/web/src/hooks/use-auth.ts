import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '@/lib/api-client';

interface SessionResponse {
  authenticated: boolean;
}

export function useAuthSession() {
  return useQuery<SessionResponse>({
    queryKey: ['auth', 'session'],
    queryFn: () => apiFetch<SessionResponse>('/auth/session'),
    retry: false,
  });
}

export function useLogin() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (secret: string) =>
      apiFetch<SessionResponse>('/auth/login', {
        method: 'POST',
        body: JSON.stringify({ secret }),
      }),
    onSuccess: () => {
      queryClient.setQueryData<SessionResponse>(['auth', 'session'], { authenticated: true });
      queryClient.invalidateQueries({ queryKey: ['auth', 'session'] });
    },
  });
}

export function useLogout() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      apiFetch<SessionResponse>('/auth/logout', {
        method: 'POST',
      }),
    onSuccess: () => {
      queryClient.clear();
    },
  });
}
