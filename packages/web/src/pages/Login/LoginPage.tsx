import { FormEvent, useState } from 'react';
import { Lock } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { useLogin } from '@/hooks/use-auth';

export function LoginPage() {
  const [secret, setSecret] = useState('');
  const login = useLogin();

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    login.mutate(secret);
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-background px-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-xl">
            <Lock className="h-5 w-5" />
            Stepyard Dashboard
          </CardTitle>
        </CardHeader>
        <CardContent>
          <form className="space-y-4" onSubmit={handleSubmit}>
            <Input
              autoFocus
              type="password"
              placeholder="API secret"
              value={secret}
              onChange={(event) => setSecret(event.target.value)}
            />
            {login.error instanceof Error && (
              <div className="rounded-md bg-destructive/10 text-destructive p-3 text-sm">
                {login.error.message}
              </div>
            )}
            <Button className="w-full" type="submit" disabled={login.isPending || secret.length === 0}>
              {login.isPending ? 'Signing in...' : 'Sign in'}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}

