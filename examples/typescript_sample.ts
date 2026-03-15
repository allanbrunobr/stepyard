/**
 * Sample TypeScript module with intentional bugs for testing Minion Engine workflows.
 * This file has type safety issues, async problems, and security vulnerabilities.
 */

// BUG: Using `any` type where specific type is possible
interface User {
  id: number;
  name: string;
  email: string;
  role: any;  // Should be a union type like 'admin' | 'user' | 'guest'
}

// BUG: Hardcoded API key (security issue)
const API_KEY = "sk-live-1234567890abcdef";

// BUG: Missing return type on exported function
export function getUsers() {
  return fetch("/api/users").then((res) => res.json());
}

// BUG: Missing `await` on async call (floating promise)
export async function deleteUser(id: number): Promise<void> {
  fetch(`/api/users/${id}`, { method: "DELETE" });
  console.log("User deleted");
}

// BUG: Untyped catch parameter and silent error swallowing
export async function fetchData(url: string) {
  try {
    const response = await fetch(url);
    return await response.json();
  } catch (e) {
    // Silent error swallowing — no logging, no re-throw
  }
}

// BUG: `==` instead of `===`
export function isAdmin(user: User): boolean {
  return user.role == "admin";
}

// BUG: JSON.parse without validation
export function parseConfig(raw: string): User {
  return JSON.parse(raw);
}

// BUG: async function that doesn't use await (unnecessary async)
export async function formatUserName(user: User): Promise<string> {
  return `${user.name} <${user.email}>`;
}

// BUG: Non-null assertion instead of proper null check
export function getUserEmail(users: User[], id: number): string {
  const user = users.find((u) => u.id === id);
  return user!.email;
}

// BUG: Type assertion that bypasses type system
export function processResponse(data: unknown): User {
  return data as User;
}

// BUG: Race condition in concurrent operations
export async function updateAllUsers(users: User[], newRole: string) {
  users.forEach(async (user) => {
    await fetch(`/api/users/${user.id}`, {
      method: "PATCH",
      body: JSON.stringify({ role: newRole }),
    });
  });
  // Returns before all updates complete
}

// BUG: Default export instead of named export (harder to refactor)
export default class UserService {
  private cache: Map<number, User> = new Map();

  // BUG: No error handling for failed fetch
  async getById(id: any): Promise<User> {
    if (this.cache.has(id)) {
      return this.cache.get(id) as User;
    }
    const resp = await fetch(`/api/users/${id}`);
    const user = await resp.json();
    this.cache.set(id, user);
    return user;
  }

  // BUG: sort() mutates original array
  getSortedUsers(users: User[]): User[] {
    return users.sort((a, b) => a.name.localeCompare(b.name));
  }
}
