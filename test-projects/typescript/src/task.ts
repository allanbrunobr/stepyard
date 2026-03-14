import { v4 as uuidv4 } from "uuid";

export type Priority = "low" | "medium" | "high" | "critical";
export type TaskStatus = "todo" | "in_progress" | "done" | "cancelled";

export interface Task {
  id: string;
  title: string;
  description: string;
  priority: Priority;
  status: TaskStatus;
  assignee: string | null;
  createdAt: Date;
  updatedAt: Date;
  tags: string[];
}

export interface CreateTaskInput {
  title: string;
  description?: string;
  priority?: Priority;
  assignee?: string;
  tags?: string[];
}

export class TaskManager {
  private tasks: Map<string, Task> = new Map();

  create(input: CreateTaskInput): Task {
    if (!input.title || input.title.trim().length === 0) {
      throw new Error("Task title is required");
    }

    const now = new Date();
    const task: Task = {
      id: uuidv4(),
      title: input.title.trim(),
      description: input.description?.trim() || "",
      priority: input.priority || "medium",
      status: "todo",
      assignee: input.assignee || null,
      createdAt: now,
      updatedAt: now,
      tags: input.tags || [],
    };

    this.tasks.set(task.id, task);
    return task;
  }

  getById(id: string): Task | undefined {
    return this.tasks.get(id);
  }

  list(filters?: { status?: TaskStatus; priority?: Priority; assignee?: string }): Task[] {
    let result = Array.from(this.tasks.values());

    if (filters?.status) {
      result = result.filter((t) => t.status === filters.status);
    }
    if (filters?.priority) {
      result = result.filter((t) => t.priority === filters.priority);
    }
    if (filters?.assignee) {
      result = result.filter((t) => t.assignee === filters.assignee);
    }

    return result.sort((a, b) => b.createdAt.getTime() - a.createdAt.getTime());
  }

  updateStatus(id: string, status: TaskStatus): Task {
    const task = this.tasks.get(id);
    if (!task) {
      throw new Error(`Task not found: ${id}`);
    }

    task.status = status;
    task.updatedAt = new Date();
    return task;
  }

  assignTo(id: string, assignee: string): Task {
    const task = this.tasks.get(id);
    if (!task) {
      throw new Error(`Task not found: ${id}`);
    }

    task.assignee = assignee;
    task.updatedAt = new Date();
    return task;
  }

  delete(id: string): boolean {
    return this.tasks.delete(id);
  }

  getStats(): { total: number; byStatus: Record<TaskStatus, number>; byPriority: Record<Priority, number> } {
    const byStatus: Record<TaskStatus, number> = { todo: 0, in_progress: 0, done: 0, cancelled: 0 };
    const byPriority: Record<Priority, number> = { low: 0, medium: 0, high: 0, critical: 0 };

    for (const task of this.tasks.values()) {
      byStatus[task.status]++;
      byPriority[task.priority]++;
    }

    return { total: this.tasks.size, byStatus, byPriority };
  }
}
