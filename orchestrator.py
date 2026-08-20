import asyncio
import sys
import argparse
from pathlib import Path
from datetime import datetime
from signal import SIGINT
import tomllib



class LaunchLogger:
    """Handles writing to both stdout and the launch.log file simultaneously."""
    def __init__(self, log_dir: str):
        log_path = Path(log_dir)
        log_path.mkdir(parents=True, exist_ok=True)

        # We append to launch.log, with a timestamp separator for new runs
        self.file_path = log_path / "launch.log"
        self.file = open(self.file_path, "a")

        header = f"\n=== Launch Started: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')} ===\n"
        self.log(header)

    def log(self, message: str):
        # Print to terminal
        sys.stdout.write(message)
        sys.stdout.flush()
        # Write to file
        self.file.write(message)
        self.file.flush()

    def close(self):
        footer = f"=== Launch Ended: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')} ===\n"
        self.log(footer)
        self.file.close()

async def read_stream(stream: asyncio.StreamReader, logger: LaunchLogger):
    """Asynchronously reads a stream and passes it to the logger."""
    while True:
        line = await stream.readline()
        if not line:
            break
        # Decode and pass through natively; nodes handle their own formatting
        logger.log(line.decode('utf-8', errors='replace'))

async def build_nodes(node_names: list[str], logger: LaunchLogger) -> bool:
    """Compiles all requested nodes in a single Cargo command."""
    cmd = ["cargo", "build"]
    for name in node_names:
        cmd.extend(["-p", name])

    logger.log(f"[Orchestrator] Running build: {' '.join(cmd)}\n")

    proc = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE
    )

    await asyncio.gather(
        read_stream(proc.stdout, logger),
        read_stream(proc.stderr, logger)
    )

    return await proc.wait() == 0

async def launch_node(node_name: str, config: dict, global_params: dict, logger: LaunchLogger, pass_args: bool = False):
    """Spawns the compiled node binary and captures its output."""
    binary_path = Path("target") / "debug" / node_name
    cmd = [str(binary_path)]

    if pass_args:
        # Merge global [params] with node-specific params
        combined_params = {**global_params, **config}

        for key, value in combined_params.items():
            if key == "priority":
                continue

            if isinstance(value, bool):
                value = str(value).lower()

            cmd.extend([f"--{key}", str(value)])

    logger.log(f"[Orchestrator] Launching {node_name}: {' '.join(cmd)}\n")

    proc = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE
    )

    try:
        await asyncio.gather(
            read_stream(proc.stdout, logger),
            read_stream(proc.stderr, logger)
        )
        await proc.wait()
    except asyncio.CancelledError:
        # If Ctrl+C is pressed, terminate the child process cleanly
        if proc.returncode is None:
            logger.log(f"[Orchestrator] Sending SIGINT to {node_name}...\n")
            proc.send_signal(SIGINT)
            # Wait a moment for the process to actually die so the pipes close
            await proc.wait()
        # raise  # Re-raise to continue the cancellation chain

async def main():
    parser = argparse.ArgumentParser(description="Rust Node Orchestrator")
    parser.add_argument("launch_file", help="Path to the TOML launch file")
    args = parser.parse_args()

    # 1. Parse the TOML file
    try:
        with open(args.launch_file, "rb") as f:
            launch_data = tomllib.load(f)
    except FileNotFoundError:
        print(f"Error: Could not find {args.launch_file}")
        sys.exit(1)

    settings = launch_data.get("settings", None)
    if settings is None:
        raise ValueError("Settings section is missing from the launch file.")
    global_params = settings.get("global", {})

    # Extract nodes (anything that isn't 'settings' or 'params')
    nodes = {k: v for k, v in launch_data.items() if k not in ["settings"]}

    if not nodes:
        print("No nodes found in launch file.")
        sys.exit(1)

    log_dir = settings.get("log_dir", "./log/")
    logger = LaunchLogger(log_dir)
    logger.log(str(nodes)+'\n')
    # 2. Build Step
    node_names = list(nodes.keys())
    success = await build_nodes(node_names, logger)
    if not success:
        logger.log("[Orchestrator] Build failed. Aborting launch.\n")
        logger.close()
        sys.exit(1)
    else:
        logger.log("[Orchestrator] Build success. Launching nodes...\n")

    # 3. Sort nodes by priority (default to 100 if missing)
    sorted_nodes = sorted(nodes.items(), key=lambda item: item[1].get("priority", 100))

    # 4. Launch Step
    launch_tasks = []
    launch_delay = settings.get("delay", 0.1) # Get delay between launches if defined
    pass_args = settings.get("pass_args", False)
    logger.log(f"[Orchestrator] Passing args: {pass_args}\n")

    for name, config in sorted_nodes:
        task = asyncio.create_task(launch_node(name, config, global_params, logger, pass_args))
        launch_tasks.append(task)
        # Small stagger delay between spawns to ensure startup order is respected
        await asyncio.sleep(launch_delay)

    logger.log("[Orchestrator] All nodes launched. Press Ctrl+C to stop.\n")

    # Wait for all nodes to finish (or run indefinitely until interrupted)
    try:
        await asyncio.gather(*launch_tasks)
    except asyncio.CancelledError:
        logger.log("[Orchestrator] Shutting down nodes...\n")
    finally:
        logger.close()

if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n[Orchestrator] Terminated by user.")