"""Tests for the user management API."""

import pytest
from app import app, users_db, validate_email, validate_user_data


@pytest.fixture
def client():
    """Create test client."""
    app.config["TESTING"] = True
    with app.test_client() as client:
        users_db.clear()
        yield client


class TestValidation:
    def test_valid_email(self):
        assert validate_email("user@example.com") is True

    def test_invalid_email_no_at(self):
        assert validate_email("userexample.com") is False

    def test_invalid_email_no_dot(self):
        assert validate_email("user@examplecom") is False

    def test_valid_user_data(self):
        valid, msg = validate_user_data({"name": "Alice", "email": "alice@test.com"})
        assert valid is True
        assert msg == ""

    def test_missing_name(self):
        valid, msg = validate_user_data({"email": "alice@test.com"})
        assert valid is False
        assert "Name" in msg

    def test_missing_email(self):
        valid, msg = validate_user_data({"name": "Alice"})
        assert valid is False
        assert "Email" in msg


class TestUserAPI:
    def test_list_users_empty(self, client):
        resp = client.get("/users")
        assert resp.status_code == 200
        assert resp.get_json() == []

    def test_create_user(self, client):
        resp = client.post("/users", json={"name": "Alice", "email": "alice@test.com"})
        assert resp.status_code == 201
        data = resp.get_json()
        assert data["name"] == "Alice"
        assert data["role"] == "user"

    def test_get_user(self, client):
        client.post("/users", json={"name": "Bob", "email": "bob@test.com"})
        resp = client.get("/users/1")
        assert resp.status_code == 200
        assert resp.get_json()["name"] == "Bob"

    def test_get_user_not_found(self, client):
        resp = client.get("/users/999")
        assert resp.status_code == 404

    def test_delete_user(self, client):
        client.post("/users", json={"name": "Charlie", "email": "charlie@test.com"})
        resp = client.delete("/users/1")
        assert resp.status_code == 204

    def test_delete_user_not_found(self, client):
        resp = client.delete("/users/999")
        assert resp.status_code == 404
