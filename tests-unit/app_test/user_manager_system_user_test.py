"""Tests for H-Gripe local workspace user management.

H-Gripe keeps the ComfyUI userdata endpoints for compatibility, but it does
not expose cloud accounts or multiple HTTP-selected users. Every request maps
to the local default workspace.
"""

import os
import tempfile
from unittest.mock import MagicMock, patch

import pytest

import folder_paths
from app.user_manager import UserManager, account_creation_disabled, default_user


@pytest.fixture
def mock_user_directory():
    """Create a temporary user directory."""
    with tempfile.TemporaryDirectory() as temp_dir:
        original_dir = folder_paths.get_user_directory()
        folder_paths.set_user_directory(temp_dir)
        yield temp_dir
        folder_paths.set_user_directory(original_dir)


@pytest.fixture
def user_manager(mock_user_directory):
    """Create a UserManager instance with the deprecated flag enabled."""
    with patch("app.user_manager.args") as mock_args:
        mock_args.multi_user = True
        yield UserManager()


def make_request(headers=None):
    request = MagicMock()
    request.headers = headers or {}
    return request


class TestLocalWorkspaceUserId:
    @pytest.mark.parametrize(
        "headers",
        [
            {},
            {"comfy-user": "default"},
            {"comfy-user": "unknown_user"},
            {"comfy-user": "__system"},
            {"comfy-user": "__cache"},
        ],
    )
    def test_headers_are_ignored(self, user_manager, headers):
        request = make_request(headers)

        assert user_manager.get_request_user_id(request) == default_user


class TestLocalWorkspaceFilepath:
    @pytest.mark.parametrize(
        "headers",
        [
            {},
            {"comfy-user": "unknown_user"},
            {"comfy-user": "__system"},
        ],
    )
    def test_user_paths_stay_under_default_workspace(
        self, user_manager, mock_user_directory, headers
    ):
        request = make_request(headers)

        path = user_manager.get_request_user_filepath(
            request,
            "workflows/test.json",
            create_dir=False,
        )

        default_root = folder_paths.get_public_user_directory(default_user)
        assert path is not None
        assert os.path.commonpath((default_root, path)) == default_root
        assert os.path.commonpath((mock_user_directory, path)) == mock_user_directory
        assert folder_paths.SYSTEM_USER_PREFIX not in os.path.relpath(path, mock_user_directory)

    def test_path_traversal_is_still_blocked(self, user_manager):
        request = make_request({"comfy-user": "__system"})

        assert user_manager.get_request_user_filepath(
            request,
            "../secret.txt",
            create_dir=False,
        ) is None

    def test_public_user_directory_still_blocks_system_direct_access(self):
        assert folder_paths.get_public_user_directory("__system") is None
        assert folder_paths.get_public_user_directory("__cache") is None
        assert folder_paths.get_public_user_directory(default_user) is not None


class TestAccountCreationDisabled:
    @pytest.mark.parametrize("name", ["Normal User", "__system", "", "   "])
    def test_add_user_is_disabled(self, user_manager, name):
        with pytest.raises(ValueError, match=account_creation_disabled):
            user_manager.add_user(name)

    def test_users_json_is_not_created(self, user_manager):
        assert not os.path.exists(user_manager.get_users_file())
