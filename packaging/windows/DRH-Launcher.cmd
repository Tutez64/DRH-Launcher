@echo off
rem Installed on Windows as `bin\DRH-Launcher.cmd`, for the modder commands.
rem `DRH-Launcher.exe` is a GUI program: typed at a prompt, the shell neither
rem waits for it nor reads its exit code. From a script cmd does both, and the
rem launcher writes to the console it was started from.
"%~dp0..\DRH-Launcher.exe" %*
