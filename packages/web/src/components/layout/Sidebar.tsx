import { NavLink } from 'react-router-dom';
import { cn } from '@/lib/utils';

const navItems = [
  { label: 'Overview', path: '/' },
  { label: 'Workflow Log', path: '/workflows' },
  { label: 'Developer Activity', path: '/developers' },
  { label: 'Cost Tracking', path: '/costs' },
];

export function Sidebar() {
  return (
    <aside className="w-60 h-screen border-r border-[hsl(var(--border))] bg-[hsl(var(--card))] flex flex-col">
      <div className="p-4 border-b border-[hsl(var(--border))]">
        <h1 className="text-lg font-bold">Minion Dashboard</h1>
      </div>
      <nav className="flex-1 p-2">
        {navItems.map((item) => (
          <NavLink
            key={item.path}
            to={item.path}
            end={item.path === '/'}
            className={({ isActive }) =>
              cn(
                'block px-3 py-2 rounded-md text-sm font-medium mb-1 transition-colors',
                isActive
                  ? 'bg-[hsl(var(--primary))] text-[hsl(var(--primary-foreground))]'
                  : 'text-[hsl(var(--muted-foreground))] hover:bg-[hsl(var(--muted))]'
              )
            }
          >
            {item.label}
          </NavLink>
        ))}
      </nav>
    </aside>
  );
}
