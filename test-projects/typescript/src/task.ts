import { v4 as uuidv4 } from "uuid";

export type Priority = "low" | "medium" | "high" | "critical";
export type TaskStatus = "todo" | "in_progress" | "done" | "cancelled" | "blocked";

export interface Task {
  id: string;
  title: string;
  description: string;
  priority: Priority;
  status: TaskStatus;
  assignee: string | null;
  createdAt: Date;
  updatedAt: Date;
  dueDate: Date | null;
  tags: string[];
  subtasks: Subtask[];
  blockedBy: string[];
}

export interface Subtask {
  id: string;
  title: string;
  completed: boolean;
}

export interface CreateTaskInput {
  title: string;
  description?: string;
  priority?: Priority;
  assignee?: string;
  tags?: string[];
  dueDate?: Date;
}

export interface UpdateTaskInput {
  title?: string;
  description?: string;
  priority?: Priority;
  assignee?: string | null;
  tags?: string[];
  dueDate?: Date | null;
}

export class TaskManager {
  private tasks: Map<string, Task> = new Map();

  create(input: CreateTaskInput): Task {
    if (!input.title || input.title.trim().length === 0) {
      throw new Error("Task title is required");
    }

    if (input.title.trim().length > 200) {
      throw new Error("Task title must be 200 characters or less");
    }

    const now = new Date();
    const task: Task = {
      id: uuidv4(),
      title: input.title.trim(),
      description: input.description?.trim() || "",
      priority: input.priority || "low",
      status: "todo",
      assignee: input.assignee || null,
      createdAt: now,
      updatedAt: now,
      dueDate: input.dueDate || null,
      tags: [...new Set(input.tags || [])], // deduplicate tags
      subtasks: [],
      blockedBy: [],
    };

    this.tasks.set(task.id, task);
    return task;
  }

  getById(id: string): Task | undefined {
    return this.tasks.get(id);
  }

  update(id: string, input: UpdateTaskInput): Task {
    const task = this.tasks.get(id);
    if (!task) {
      throw new Error(`Task not found: ${id}`);
    }

    if (input.title !== undefined) {
      if (input.title.trim().length === 0) {
        throw new Error("Task title cannot be empty");
      }
      task.title = input.title.trim();
    }
    if (input.description !== undefined) task.description = input.description.trim();
    if (input.priority !== undefined) task.priority = input.priority;
    if (input.assignee !== undefined) task.assignee = input.assignee;
    if (input.tags !== undefined) task.tags = [...new Set(input.tags)];
    if (input.dueDate !== undefined) task.dueDate = input.dueDate;

    task.updatedAt = new Date();
    return task;
  }

  list(filters?: {
    status?: TaskStatus;
    priority?: Priority;
    assignee?: string;
    tag?: string;
    overdue?: boolean;
  }): Task[] {
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
    if (filters?.tag) {
      result = result.filter((t) => t.tags.includes(filters.tag!));
    }
    if (filters?.overdue) {
      const now = new Date();
      result = result.filter(
        (t) => t.dueDate !== null && t.dueDate < now && t.status !== "done" && t.status !== "cancelled"
      );
    }

    return result.sort((a, b) => {
      // Sort by priority first (critical > high > medium > low)
      const priorityOrder: Record<Priority, number> = { critical: 4, high: 3, medium: 2, low: 1 };
      const pDiff = priorityOrder[b.priority] - priorityOrder[a.priority];
      if (pDiff !== 0) return pDiff;
      // Then by creation date (newest first)
      return b.createdAt.getTime() - a.createdAt.getTime();
    });
  }

  updateStatus(id: string, status: TaskStatus): Task {
    const task = this.tasks.get(id);
    if (!task) {
      throw new Error(`Task not found: ${id}`);
    }

    // Validate status transitions
    if (status === "in_progress" && task.blockedBy.length > 0) {
      const stillBlocked = task.blockedBy.filter((bid) => {
        const blocker = this.tasks.get(bid);
        return blocker && blocker.status !== "done" && blocker.status !== "cancelled";
      });
      if (stillBlocked.length > 0) {
        throw new Error(`Task is blocked by: ${stillBlocked.join(", ")}`);
      }
    }

    task.status = status;
    task.updatedAt = new Date();
    return task;
  }

  addBlocker(taskId: string, blockerTaskId: string): Task {
    const task = this.tasks.get(taskId);
    if (!task) throw new Error(`Task not found: ${taskId}`);
    if (!this.tasks.has(blockerTaskId)) throw new Error(`Blocker task not found: ${blockerTaskId}`);
    if (taskId === blockerTaskId) throw new Error("A task cannot block itself");

    if (!task.blockedBy.includes(blockerTaskId)) {
      task.blockedBy.push(blockerTaskId);
      if (task.status === "todo") {
        task.status = "blocked";
      }
      task.updatedAt = new Date();
    }
    return task;
  }

  addSubtask(taskId: string, title: string): Task {
    const task = this.tasks.get(taskId);
    if (!task) throw new Error(`Task not found: ${taskId}`);

    task.subtasks.push({
      id: uuidv4(),
      title: title.trim(),
      completed: false,
    });
    task.updatedAt = new Date();
    return task;
  }

  toggleSubtask(taskId: string, subtaskId: string): Task {
    const task = this.tasks.get(taskId);
    if (!task) throw new Error(`Task not found: ${taskId}`);

    const subtask = task.subtasks.find((s) => s.id === subtaskId);
    if (!subtask) throw new Error(`Subtask not found: ${subtaskId}`);

    subtask.completed = !subtask.completed;
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
    // Remove from other tasks' blockedBy lists
    if (this.tasks.has(id)) {
      for (const task of this.tasks.values()) {
        task.blockedBy = task.blockedBy.filter((bid) => bid !== id);
        // If unblocked, revert from blocked to todo
        if (task.status === "blocked" && task.blockedBy.length === 0) {
          task.status = "todo";
        }
      }
    }
    return this.tasks.delete(id);
  }

  getStats(): {
    total: number;
    byStatus: Record<TaskStatus, number>;
    byPriority: Record<Priority, number>;
    overdue: number;
    completionRate: number;
  } {
    const byStatus: Record<TaskStatus, number> = {
      todo: 0, in_progress: 0, done: 0, cancelled: 0, blocked: 0,
    };
    const byPriority: Record<Priority, number> = { low: 0, medium: 0, high: 0, critical: 0 };
    let overdue = 0;
    const now = new Date();

    for (const task of this.tasks.values()) {
      byStatus[task.status]++;
      byPriority[task.priority]++;
      if (task.dueDate && task.dueDate < now && task.status !== "done" && task.status !== "cancelled") {
        overdue++;
      }
    }

    const total = this.tasks.size;
    const completionRate = total > 0 ? byStatus.done / total : 0;

    return { total, byStatus, byPriority, overdue, completionRate };
  }
}
