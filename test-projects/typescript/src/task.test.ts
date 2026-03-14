import { TaskManager, Task, CreateTaskInput } from "./task";

describe("TaskManager", () => {
  let manager: TaskManager;

  beforeEach(() => {
    manager = new TaskManager();
  });

  describe("create", () => {
    it("should create a task with defaults", () => {
      const task = manager.create({ title: "Buy groceries" });
      expect(task.title).toBe("Buy groceries");
      expect(task.priority).toBe("medium");
      expect(task.status).toBe("todo");
      expect(task.assignee).toBeNull();
      expect(task.tags).toEqual([]);
    });

    it("should create a task with all fields", () => {
      const input: CreateTaskInput = {
        title: "Fix login bug",
        description: "Users can't login with SSO",
        priority: "critical",
        assignee: "alice",
        tags: ["bug", "auth"],
      };
      const task = manager.create(input);
      expect(task.description).toBe("Users can't login with SSO");
      expect(task.priority).toBe("critical");
      expect(task.assignee).toBe("alice");
      expect(task.tags).toEqual(["bug", "auth"]);
    });

    it("should throw on empty title", () => {
      expect(() => manager.create({ title: "" })).toThrow("Task title is required");
    });

    it("should trim whitespace from title", () => {
      const task = manager.create({ title: "  Hello World  " });
      expect(task.title).toBe("Hello World");
    });
  });

  describe("getById", () => {
    it("should return task by id", () => {
      const created = manager.create({ title: "Test" });
      const found = manager.getById(created.id);
      expect(found).toEqual(created);
    });

    it("should return undefined for unknown id", () => {
      expect(manager.getById("nonexistent")).toBeUndefined();
    });
  });

  describe("list", () => {
    beforeEach(() => {
      manager.create({ title: "Task A", priority: "high", assignee: "alice" });
      manager.create({ title: "Task B", priority: "low", assignee: "bob" });
      manager.create({ title: "Task C", priority: "high", assignee: "alice" });
    });

    it("should list all tasks", () => {
      expect(manager.list()).toHaveLength(3);
    });

    it("should filter by priority", () => {
      const high = manager.list({ priority: "high" });
      expect(high).toHaveLength(2);
    });

    it("should filter by assignee", () => {
      const alice = manager.list({ assignee: "alice" });
      expect(alice).toHaveLength(2);
    });
  });

  describe("updateStatus", () => {
    it("should update status", () => {
      const task = manager.create({ title: "Do stuff" });
      const updated = manager.updateStatus(task.id, "in_progress");
      expect(updated.status).toBe("in_progress");
    });

    it("should throw on unknown task", () => {
      expect(() => manager.updateStatus("bad-id", "done")).toThrow("Task not found");
    });
  });

  describe("delete", () => {
    it("should delete existing task", () => {
      const task = manager.create({ title: "Delete me" });
      expect(manager.delete(task.id)).toBe(true);
      expect(manager.getById(task.id)).toBeUndefined();
    });

    it("should return false for unknown task", () => {
      expect(manager.delete("nope")).toBe(false);
    });
  });

  describe("getStats", () => {
    it("should return correct stats", () => {
      manager.create({ title: "A", priority: "high" });
      manager.create({ title: "B", priority: "low" });
      const taskC = manager.create({ title: "C", priority: "high" });
      manager.updateStatus(taskC.id, "done");

      const stats = manager.getStats();
      expect(stats.total).toBe(3);
      expect(stats.byPriority.high).toBe(2);
      expect(stats.byStatus.done).toBe(1);
      expect(stats.byStatus.todo).toBe(2);
    });
  });
});
