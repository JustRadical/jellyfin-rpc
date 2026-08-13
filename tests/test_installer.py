import contextlib
import io
import os
import platform
from pathlib import Path
import runpy
import subprocess
import sys
import time
import unittest
from unittest import mock
from xml.etree import ElementTree


INSTALLER = Path(__file__).parents[1] / "scripts" / "installer.py"


class WindowsServiceConfigTests(unittest.TestCase):
    def test_configuration_paths_with_spaces_remain_intact(self):
        appdata = r"C:\Users\Charlie Harper\AppData\Roaming"
        config_directory = appdata + "\\jellyfin-rpc\\"
        responses = iter(
            [
                "http://localhost:8096",
                "api-key",
                "Charlie",
                "n",
                "n",
                "n",
                "",
                "n",
                "n",
                "",
                "",
                "n",
                "n",
                "",
                "y",
            ]
        )
        completed_process = subprocess.CompletedProcess([], 0)
        file_open = mock.mock_open()

        with (
            mock.patch.dict(os.environ, {"APPDATA": appdata}),
            mock.patch.object(os.path, "isfile", return_value=False),
            mock.patch.object(platform, "system", return_value="Windows"),
            mock.patch.object(subprocess, "run", return_value=completed_process),
            mock.patch.object(sys, "argv", [str(INSTALLER)]),
            mock.patch.object(time, "sleep"),
            mock.patch("builtins.input", side_effect=responses),
            mock.patch("builtins.open", file_open),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            runpy.run_path(str(INSTALLER), run_name="__main__")

        service_config = file_open.return_value.write.call_args_list[-1].args[0]
        arguments = ElementTree.fromstring(service_config).findtext("arguments")

        expected_arguments = subprocess.list2cmdline(
            [
                "-c",
                config_directory + "main.json",
                "-i",
                config_directory + "urls.json",
            ]
        )
        self.assertEqual(arguments, expected_arguments)


if __name__ == "__main__":
    unittest.main()
