import { NavLink } from 'react-router-dom';
import { LayoutDashboard, List, Users, DollarSign } from 'lucide-react';
import { cn } from '@/lib/utils';

const navItems = [
  { to: '/', label: 'Overview', icon: LayoutDashboard },
  { to: '/workflows', label: 'Workflows', icon: List },
  { to: '/developers', label: 'Developers', icon: Users },
  { to: '/costs', label: 'Costs', icon: DollarSign },
];

export function Sidebar() {
  return (
    <aside className="w-60 border-r bg-card min-h-screen p-4">
      <h2 className="text-lg font-bold mb-6 px-2">Minion Dashboard</h2>
      <nav className="flex flex-col gap-1">
        {navItems.map(({ to, label, icon: Icon }) => (
          <NavLink
            key={to}
            to={to}
            className={({ isActive }) =>
              cn(
                'flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors hover:bg-accent',
                isActive && 'bg-accent text-accent-foreground',
              )
            }
          >
            <Icon className="h-4 w-4" />
            {label}
          </NavLink>
        ))}
      </nav>
    </aside>
  );
}
