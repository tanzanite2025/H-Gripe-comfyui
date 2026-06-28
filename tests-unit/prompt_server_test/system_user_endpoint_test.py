"""HTTP endpoint tests for H-Gripe local workspace mode.

The legacy ComfyUI user routes remain available, but account creation and
HTTP-selected users are disabled. Requests with any comfy-user header still
operate on the single local default workspace.
"""

import os
from unittest.mock import patch

import pytest
from aiohttp import web

import folder_paths
from app.user_manager import UserManager, account_creation_disabled, default_user


@pytest.fixture
def mock_user_directory(tmp_path):
    """Create a temporary user directory."""
    original_dir = folder_paths.get_user_directory()
    folder_paths.set_user_directory(str(tmp_path))
    yield tmp_path
    folder_paths.set_user_directory(original_dir)


@pytest.fixture
def user_manager_local(mock_user_directory):
    """Create UserManager while proving the deprecated multi-user flag is ignored."""
    with patch("app.user_manager.args") as mock_args:
        mock_args.multi_user = True
        yield UserManager()


@pytest.fixture
def app_local(user_manager_local):
    """Create app with local workspace routes."""
    app = web.Application()
    routes = web.RouteTableDef()
    user_manager_local.add_routes(routes)
    app.add_routes(routes)
    return app


def default_dir(mock_user_directory):
    path = mock_user_directory / default_user
    path.mkdir(exist_ok=True)
    return path


def system_dir(mock_user_directory, name="system"):
    path = mock_user_directory / f"{folder_paths.SYSTEM_USER_PREFIX}{name}"
    path.mkdir(exist_ok=True)
    return path


class TestLocalWorkspaceUsersEndpoint:
    @pytest.mark.asyncio
    async def test_get_users_reports_local_workspace(self, aiohttp_client, app_local):
        client = await aiohttp_client(app_local)

        resp = await client.get("/users")

        assert resp.status == 200
        data = await resp.json()
        assert data["storage"] == "server"
        assert data["mode"] == "local_workspace"
        assert data["default"] == default_user
        assert data["users"] == {default_user: "Local Workspace"}

    @pytest.mark.asyncio
    @pytest.mark.parametrize("username", ["Normal User", "__system", ""])
    async def test_post_users_rejects_all_account_creation(
        self,
        aiohttp_client,
        app_local,
        username,
    ):
        client = await aiohttp_client(app_local)

        resp = await client.post("/users", json={"username": username})

        assert resp.status == 400
        data = await resp.json()
        assert data["error"] == account_creation_disabled


class TestHttpHeadersCannotSelectSystemUsers:
    @pytest.mark.asyncio
    async def test_get_userdata_with_system_header_reads_default_workspace(
        self,
        aiohttp_client,
        app_local,
        mock_user_directory,
    ):
        (system_dir(mock_user_directory) / "secret.txt").write_text("sensitive data")
        client = await aiohttp_client(app_local)

        resp = await client.get(
            "/userdata/secret.txt",
            headers={"comfy-user": "__system"},
        )

        assert resp.status == 404

    @pytest.mark.asyncio
    async def test_post_userdata_with_system_header_writes_default_workspace(
        self,
        aiohttp_client,
        app_local,
        mock_user_directory,
    ):
        sys_dir = system_dir(mock_user_directory)
        client = await aiohttp_client(app_local)

        resp = await client.post(
            "/userdata/test.txt",
            headers={"comfy-user": "__system"},
            data=b"local content",
        )

        assert resp.status == 200
        assert (mock_user_directory / default_user / "test.txt").read_bytes() == b"local content"
        assert not (sys_dir / "test.txt").exists()

    @pytest.mark.asyncio
    async def test_delete_userdata_with_system_header_deletes_only_default_file(
        self,
        aiohttp_client,
        app_local,
        mock_user_directory,
    ):
        default_root = default_dir(mock_user_directory)
        (default_root / "note.txt").write_text("default")
        sys_dir = system_dir(mock_user_directory)
        system_file = sys_dir / "note.txt"
        system_file.write_text("system")
        client = await aiohttp_client(app_local)

        resp = await client.delete(
            "/userdata/note.txt",
            headers={"comfy-user": "__system"},
        )

        assert resp.status == 204
        assert not (default_root / "note.txt").exists()
        assert system_file.exists()

    @pytest.mark.asyncio
    async def test_move_userdata_with_system_header_moves_only_default_file(
        self,
        aiohttp_client,
        app_local,
        mock_user_directory,
    ):
        default_root = default_dir(mock_user_directory)
        (default_root / "source.txt").write_text("default")
        sys_dir = system_dir(mock_user_directory)
        (sys_dir / "source.txt").write_text("system")
        client = await aiohttp_client(app_local)

        resp = await client.post(
            "/userdata/source.txt/move/dest.txt",
            headers={"comfy-user": "__system"},
        )

        assert resp.status == 200
        assert not (default_root / "source.txt").exists()
        assert (default_root / "dest.txt").read_text() == "default"
        assert (sys_dir / "source.txt").read_text() == "system"
        assert not (sys_dir / "dest.txt").exists()

    @pytest.mark.asyncio
    async def test_v2_userdata_with_system_header_lists_default_workspace(
        self,
        aiohttp_client,
        app_local,
        mock_user_directory,
    ):
        default_root = default_dir(mock_user_directory)
        (default_root / "visible.txt").write_text("default")
        (system_dir(mock_user_directory) / "secret.txt").write_text("system")
        client = await aiohttp_client(app_local)

        resp = await client.get(
            "/v2/userdata",
            headers={"comfy-user": "__system"},
        )

        assert resp.status == 200
        paths = {item["path"] for item in await resp.json()}
        assert "visible.txt" in paths
        assert "secret.txt" not in paths


class TestInternalSystemUserApiStillExists:
    def test_internal_api_can_access_system_user(self, mock_user_directory):
        system_path = folder_paths.get_system_user_directory("mynode_config")

        assert system_path is not None
        assert "__mynode_config" in system_path

        os.makedirs(system_path, exist_ok=True)
        config_file = os.path.join(system_path, "settings.json")
        with open(config_file, "w") as f:
            f.write('{"api_key": "secret"}')

        assert os.path.exists(config_file)

    def test_public_user_directory_blocks_system_direct_access(self):
        assert folder_paths.get_public_user_directory("__system") is None
        assert folder_paths.get_public_user_directory("__cache") is None
        assert folder_paths.get_public_user_directory(default_user) is not None
