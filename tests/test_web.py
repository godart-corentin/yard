import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock


class FakeResponse:
    def __init__(self, status=200):
        self.status = status

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

    def getcode(self):
        return self.status

    def read(self, _limit=None):
        return b"ok"


class YardWebTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        root = Path(self.temp.name)
        self.projects = root / "projects"
        self.state = root / "state"
        self.projects.mkdir()
        self.state.mkdir()

        os.environ["YARD_PROJECTS_DIR"] = str(self.projects)
        os.environ["YARD_STATE_DIR"] = str(self.state)
        module_path = Path(__file__).parents[1] / "web" / "server" / "yard_web.py"
        spec = importlib.util.spec_from_file_location("yard_web_test", module_path)
        self.web = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(self.web)

    def tearDown(self):
        self.temp.cleanup()

    def test_loads_health_url_and_current_release(self):
        (self.projects / "hello.toml").write_text(
            '[deployment]\nhealth_url = "https://example.test/health"\n',
            encoding="utf-8",
        )
        (self.state / "hello.json").write_text(
            json.dumps(
                {
                    "current": {
                        "revision": "abcdef1234567890",
                        "tag": "abcdef123456",
                        "deployed_at_unix": 123,
                    },
                    "previous": None,
                }
            ),
            encoding="utf-8",
        )

        projects = self.web.load_projects()
        self.assertEqual(len(projects), 1)
        self.assertEqual(projects[0]["name"], "hello")
        self.assertEqual(projects[0]["health_url"], "https://example.test/health")
        self.assertEqual(projects[0]["release"]["tag"], "abcdef123456")

    def test_project_without_health_url_is_unknown(self):
        project = {"name": "hello", "health_url": None, "release": None}
        result = self.web.check_project(project)
        self.assertEqual(result["status"], "unknown")
        self.assertIn("No deployment.health_url", result["error"])

    def test_successful_health_check_is_operational(self):
        project = {
            "name": "hello",
            "health_url": "https://example.test/health",
            "release": None,
        }
        with mock.patch.object(self.web.urllib.request, "urlopen", return_value=FakeResponse(200)):
            result = self.web.check_project(project)
        self.assertEqual(result["status"], "operational")
        self.assertEqual(result["http_status"], 200)
        self.assertIsInstance(result["latency_ms"], int)

    def test_overall_status_degrades_when_one_project_is_down(self):
        status = self.web.overall_status(
            [
                {"status": "operational"},
                {"status": "down"},
            ]
        )
        self.assertEqual(status, "degraded")


if __name__ == "__main__":
    unittest.main()
