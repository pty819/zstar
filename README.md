# zstar - High-Performance Parallel Archiver

`zstar` is a modern, blazingly fast command-line tool written in Rust for compressing and decompressing directories using the `.tar.zst` format. It is designed to saturate high-speed NVMe storage and multi-core CPUs.

[中文文档 (Chinese Documentation)](#zstar---高性能并行归档工具)

## Key Features

*   **⚡️ Extreme Performance**:
    *   **Parallel Scanning**: Fast directory traversal.
    *   **Parallel Multi-threaded I/O**: Reads files concurrently (default: CPU core count).
    *   **Async io_uring (Linux)**: Automatically enables `io_uring` on Linux Kernels (6.0+) for high-concurrency zero-overhead I/O.
    *   **Zstd Multithreading**: Parallel compression blocks.
*   **🛡️ Robust & Correct**:
    *   **Hardlink Deduplication**: Detects hardlinks and stores them efficiently (saving space).
    *   **Symlink & Permission Preservation**: Full support for Unix permissions and symlinks.
    *   **Error Resilience**: Optional `--ignore-failed-read` to skip unreadable files without crashing.
*   **🧠 Memory Efficient**: Smart buffer pooling and large-file streaming preventing OOM on huge files.
*   **Cross-Platform**: Works on Linux, macOS, and Windows (with permission simulation).

## Build & Compilation

Ensure you have Rust installed (via [rustup](https://rustup.rs/)).

### Linux
Prerequisites: `build-essential` (GCC, Make) for compiling zstd C dependencies.
```bash
# Ubuntu/Debian
sudo apt update && sudo apt install build-essential

# Build
cargo build --release
```
*Note: To use the `io_uring` feature, you must run the binary on a Linux Kernel >= 6.0. The build itself works on older kernels.*

### macOS
Prerequisites: Xcode Command Line Tools.
```bash
xcode-select --install
cargo build --release
```

### Windows
Prerequisites: C++ One-Click Build Tools (Visual Studio Build Tools).
```powershell
# In PowerShell or CMD
cargo build --release
```
The resulting binary will be at `.\target\release\zstar.exe`. Note that `zstar` on Windows will automatically simulate Unix permissions (755/644) so archives are usable on Linux.

## Usage

### compress (pack)

Pack a directory into an archive.

```bash
# Basic usage
./zstar pack ./my_data

# Specify output filename
./zstar pack ./my_data -o backup.tar.zst

# High compression (Level 10), explicit threads, ignore read errors
./zstar pack ./my_data --level 10 --threads 16 --ignore-failed-read

# Disable long-distance matching (enabled by default)
./zstar pack ./my_data --no-long
```

**Options:**
*   `-o, --output <PATH>`: Output file (defaults to `<DIR>.tar.zst`).
*   `-l, --level <NUM>`: Compression level (1-22, default: 3).
*   `-t, --threads <NUM>`: Number of I/O and compression threads (default: all cores).
*   `--ignore-failed-read`: Skip files with errors (e.g., Permission Denied) instead of aborting.
*   `--no-long`: Disable Zstd Long Distance Matching.

### Decompress (unpack)

Unpack an archive to a directory.

```bash
# Unpack to current directory
./zstar unpack backup.tar.zst

# Unpack to specific folder
./zstar unpack backup.tar.zst -o ./restore_path
```

## Technical Architecture

`zstar` employs a pipelined, multi-stage, multi-threaded architecture to maximize throughput.

### 1. Parallel Pipeline

Data flows through the system in three stages connected by bounded channels (Backpressure):

1.  **Scanner Phase (Thread 1)**:
    *   Uses `jwalk` to traverse the directory tree in parallel.
    *   Sends discovered paths to the **Path Channel**.

2.  **Reader Phase (Threads: N)**:
    *   **Linux (Kernel 6.0+)**: Automatically switches to a **Single-Threaded Async Worker** using `tokio-uring`. It dispatches up to 128 concurrent read operations to the kernel's Submission Queue (SQ), achieving massive I/O depth with zero syscall overhead.
    *   **Other OS (macOS/Windows/Old Linux)**: Spawns a pool of worker threads (default: CPU cores). Each thread picks a path, reads the file (using Buffer Pooling), and sends the data to the **Content Channel**.
    *   *Hardlink Optimization*: Uses a concurrent `DashMap` to track generic `(Dev, Inode)` pairs. If a duplicate inode is found, it emits a `HardLink` entry instead of reading file content.

3.  **Writer Phase (Main Thread)**:
    *   Receives file data/metadata from **Content Channel**.
    *   Constructs the TAR stream sequentially (Tar format requirement).
    *   Feeds the stream into the **Parallel Zstd Encoder** (which handles compression on auxiliary threads).
    *   Writes final bytes to disk.

---

# zstar - 高性能并行归档工具

`zstar` 是一个使用 Rust 编写的现代化、极速命令行工具，用于将目录压缩为 `.tar.zst` 格式。它的设计目标是榨干 NVMe 高速存储和多核 CPU 的性能。

## 核心特性

*   **⚡️ 极致性能**:
    *   **并行扫描**: 多线程快速遍历目录树。
    *   **并行多线程 I/O**: 并发读取文件（默认使用所有 CPU 核心）。
    *   **Async io_uring (Linux)**: 在 Linux Kernel 6.0+ 上自动启用 `io_uring`，实现高并发、零系统调用开销的异步 I/O。
    *   **Zstd 多线程压缩**: 并行块压缩。
*   **🛡️ 健壮与正确性**:
    *   **硬链接重删**: 自动检测硬链接并高效存储（节省空间）。
    *   **符号链接与权限保留**: 完美支持 Unix 权限位和 Symbolic Links。
    *   **错误容忍**: 可选 `--ignore-failed-read` 跳过无法读取的文件（如权限拒绝），防止整个任务崩溃。
*   **🧠 内存高效**: 智能缓冲池（Buffer Pooling）和大文件流式传输，防止大文件导致 OOM（内存溢出）。
*   **跨平台**: 支持 Linux, macOS, 和 Windows（Windows 下会自动模拟 Unix 权限）。

## 编译与构建

请确保已安装 Rust 环境 (通过 [rustup](https://rustup.rs/))。

### Linux
前置要求: `build-essential` (GCC, Make) 用于编译 zstd 的 C 依赖。
```bash
# Ubuntu/Debian
sudo apt update && sudo apt install build-essential

# 编译
cargo build --release
```
*注意: 要使用 `io_uring` 特性，您必须在 Linux Kernel >= 6.0 的系统上运行该二进制文件。构建过程本身兼容旧版内核。*

### macOS
前置要求: Xcode Command Line Tools.
```bash
xcode-select --install
cargo build --release
```

### Windows
前置要求: C++ 生成工具 (Visual Studio Build Tools).
```powershell
# 在 PowerShell 或 CMD 中运行
cargo build --release
```
生成的二进制文件位于 `.\target\release\zstar.exe`。注意：Windows 版会自动将权限模拟为 Unix 标准 (755/644)，确保生成的压缩包在 Linux 上解压可用。

## 使用指南

### 压缩 (pack)

将目录打包为存档文件。

```bash
# 基础用法
./zstar pack ./my_data

# 指定输出文件名
./zstar pack ./my_data -o backup.tar.zst

# 高压缩率 (Level 10), 指定线程数, 忽略读取错误
./zstar pack ./my_data --level 10 --threads 16 --ignore-failed-read

# 禁用长距离匹配 (默认开启)
./zstar pack ./my_data --no-long
```

**选项参数:**
*   `-o, --output <PATH>`: 输出文件路径 (默认为 `<DIR>.tar.zst`).
*   `-l, --level <NUM>`: 压缩等级 (1-22, 默认: 3).
*   `-t, --threads <NUM>`: I/O 和压缩线程数 (默认: 所有核心).
*   `--ignore-failed-read`: 跳过读取错误的文件（如权限不足）而不终止程序。
*   `--no-long`: 禁用 Zstd 长距离匹配 (Long Distance Matching)。

### 解压 (unpack)

将压缩包解压到目录。

```bash
# 解压到当前目录
./zstar unpack backup.tar.zst

# 解压到指定目录
./zstar unpack backup.tar.zst -o ./restore_path
```

## 技术架构

`zstar` 采用流水线（Pipeline）、多阶段、多线程的架构以最大化吞吐量。

### 1. 并行流水线

数据流经三个阶段，并通过有界通道（Bounded Channels）连接以实现背压（Backpressure）：

1.  **扫描阶段 (Scanner, 线程 1)**:
    *   使用 `jwalk` 并行遍历目录树。
    *   将发现的文件路径发送到 **Path Channel**。

2.  **读取阶段 (Reader, 线程数: N)**:
    *   **Linux (Kernel 6.0+)**: 自动切换为 **单线程异步 Worker**，使用 `tokio-uring`。它向内核的提交队列 (SQ) 调度最多 128 个并发读取操作，实现极高的 I/O 深度且无系统调用开销。
    *   **其他操作系统 (macOS/Windows/Old Linux)**: 启动工作线程池（默认与 CPU 核数相同）。每个线程获取路径，读取文件（使用缓冲池），并将数据发送到 **Content Channel**。
    *   *硬链接优化*: 使用并发 `DashMap` 追踪 `(设备号, Inode)` 对。如果发现重复的 Inode，则生成 `HardLink` 条目，而不重复读取文件内容。

3.  **写入阶段 (Writer, 主线程)**:
    *   从 **Content Channel** 接收文件数据/元数据。
    *   顺序构建 TAR 流（Tar 格式要求顺序写入）。
    *   将流通过管道送入 **并行 Zstd 编码器**（在辅助线程上处理压缩）。
    *   将最终字节写入磁盘。

### 2. 代码结构

项目高度模块化，清晰易维护：

*   **`src/main.rs`**: 入口点。解析 CLI 参数 (使用 `clap`) 并调度命令模块。
*   **`src/cli.rs`**: 命令行接口定义。
*   **`src/commands/`**:
    *   `pack.rs`: 实现 "同步" 线程池读取流水线。
    *   `pack_uring.rs`: 实现 Linux 专用的 `io_uring` 异步读取器。
    *   `unpack.rs`: 解压逻辑。
*   **`src/utils/`**:
    *   `mod.rs`: 跨平台文件系统辅助函数（Windows 权限模拟逻辑等）。
    *   `kernel_version.rs`: Linux 内核版本运行时检测。

### 3. 核心优化

*   **缓冲池 (Buffer Pooling)**: 循环利用内存缓冲区 (`Vec<u8>`)，避免处理大量小文件时的频繁内存分配。
*   **大文件流式传输**: 大于 1MB 的文件直接流式通过管道（绕过缓冲池），保持低内存占用。
*   **内核感知**: 运行时检测 `io_uring` 能力，在旧系统上保持兼容性的同时，在现代化 Linux 服务器上发挥最大性能。
