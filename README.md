# zstar - High-Performance Parallel Archiver

`zstar` is a modern, blazingly fast command-line tool written in Rust for compressing and decompressing directories using the `.tar.zst` format. It is designed to saturate high-speed NVMe storage and multi-core CPUs.

[中文文档 (Chinese Documentation)](#zstar---高性能并行归档工具)

## Key Features


*   **⚡️ Extreme Performance**:
    *   **Parallel Scanning**: Fast directory traversal.
    *   **Parallel Multi-threaded I/O**: Reads files concurrently (default: CPU core count).
    *   **Async io_uring (Linux)**: Automatically enables `io_uring` on Linux Kernels (6.0+) for high-concurrency zero-overhead I/O.
    *   **Parallel Unpacking**: 3-Stage pipelined extraction with smart directory caching.
    *   **Zstd Multithreading**: Parallel compression blocks.
*   **🛡️ Robust & Correct**:
    *   **Hardlink Deduplication**: Detects hardlinks and stores them efficiently (saving space).
    *   **Symlink & Permission Preservation**: Full support for Unix permissions and symlinks.
    *   **Deferred Metadata Application**: Solves the "Directory Mtime Paradox" by restoring stamps after file extraction.
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

# Unpack to specific folder with 8 threads
./zstar unpack backup.tar.zst -o ./restore_path -t 8
```

**Options:**
*   `-o, --output <PATH>`: Output directory (defaults to current directory).
*   `-t, --threads <NUM>`: Number of extraction threads (default: all cores).

## Architecture & Design Philosophy

`zstar` is engineered to be bound only by hardware limits (NVMe I/O or RAM bandwidth), employing a pipelined, multi-stage, multi-threaded architecture.

### 1. Packing: The "Saturation" Pipeline

The packing process uses a **Producer-Consumer** model with bounded channels for backpressure, preventing memory explosion even if one stage is faster than others.

1.  **Scanner Phase (Thread 1)**:
    *   Uses `jwalk` for parallel directory traversal.
    *   Feeds paths into the **Path Channel**.

2.  **Reader Phase (Threads: N)**:
    *   **Strategy A: Thread Pool (Universal)**: N threads race to grab paths. They use **Buffer Pooling** (recycling `Vec<u8>`) to avoid allocation overhead for small files.
    *   **Strategy B: io_uring (Linux 6.0+)**: A single thread submits up to 128 concurrent SQE (Submission Queue Entries) to the kernel. This achieves massive queue depth with zero syscall overhead (no context switches per read).
    *   **Hardlink Detection**: A concurrent `DashMap` tracks `(Dev, Inode)`. Duplicate inodes emit metadata-only entries, saving space and time.

3.  **Writer Phase (Main Thread)**:
    *   Collects data from **Content Channel**.
    *   Serializes into TAR format.
    *   Streams directly to the **Parallel Zstd Encoder** (which uses its own thread pool for block-level compression).

### 2. Unpacking: The "Correctness" Pipeline

Unpacking is trickier than packing due to race conditions (creating a file in a directory updates the directory's timestamp). `zstar` uses a **3-Stage Barrier** architecture to guarantee performance and correctness.

1.  **Parallel Extraction (Stage I)**:
    *   **Main Thread**: Streams the Zstd archive, parses Tar headers.
        *   *Small Files*: Read into memory -> Send to Worker.
        *   *Large Files (>10MB)*: Stream directly to disk (prevents OOM).
    *   **Workers**: Pop files and write them in parallel.
    *   **Optimization - Local Directory Cache**: Each worker remembers created directories. This eliminates 90%+ of redundant `mkdir` syscalls for sequential archives.

2.  **Hardlink Barrier (Stage II)**:
    *   Hardlinks are deferred until **all regular files are on disk**. This prevents "race conditions" where a link is created before its target exists.

3.  **Metadata Restoration (Stage III)**:
    *   **The "Mtime Paradox"**: Modifying a directory (adding a file) updates its `mtime`.
    *   **Solution**: Directory metadata (permissions, timestamps) is applied **Deferredly** and **Reverse-Order** (Deepest -> Shallowest) at the very end.

### 3. Core Safety Features
*   **Path Sanitization**: Prevents "Zip-Slip" attacks (absolute paths or `..` traversals).
*   **Cross-Platform ACLs**: Approximates Unix permissions on Windows to ensure archives remain usable across OS boundaries.

---

# zstar - 高性能并行归档工具

`zstar` 是一个使用 Rust 编写的现代化、极速命令行工具，用于将目录压缩为 `.tar.zst` 格式。它的设计目标是榨干 NVMe 高速存储和多核 CPU 的性能。

## 核心特性

*   **⚡️ 极致性能**:
    *   **并行扫描**: 多线程快速遍历目录树。
    *   **并行多线程 I/O**: 并发读取文件（默认使用所有 CPU 核心）。
    *   **Async io_uring (Linux)**: 在 Linux Kernel 6.0+ 上自动启用 `io_uring`，实现高并发、零系统调用开销的异步 I/O。
    *   **并行解压**: 三阶段流水线解压，配合智能目录缓存。
    *   **Zstd 多线程压缩**: 并行块压缩。
*   **🛡️ 健壮与正确性**:
    *   **硬链接重删**: 自动检测硬链接并高效存储（节省空间）。
    *   **符号链接与权限保留**: 完美支持 Unix 权限位和 Symbolic Links。
    *   **延迟元数据应用**: 解决 "目录时间戳悖论"，确保父目录时间戳不被子文件写入破坏。
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

# 解压到指定目录, 使用 8 个线程
./zstar unpack backup.tar.zst -o ./restore_path -t 8
```

**选项参数:**
*   `-o, --output <PATH>`: 输出目录 (默认为当前目录)。
*   `-t, --threads <NUM>`: 解压并行线程数 (默认: 所有核心)。

## 架构与设计理念

`zstar` 采用流水线（Pipeline）、多阶段、多线程的架构，目标是仅受限于硬件物理瓶颈（NVMe 带宽或 RAM 速度）。

### 1. 压缩架构：饱和式流水线

打包过程采用 **生产者-消费者** 模型，配合有界通道（Backpressure），防止内存爆炸。

1.  **扫描阶段 (Scanner, 线程 1)**:
    *   使用 `jwalk` 并行遍历目录树。
    *   将发现的文件路径发送到 **Path Channel**。

2.  **读取阶段 (Reader, 线程数: N)**:
    *   **策略 A: 线程池 (通用)**: N 个线程竞争获取路径。使用 **缓冲池 (Buffer Pooling)** 复用 `Vec<u8>`，避免小文件的内存分配开销。
    *   **策略 B: io_uring (Linux 6.0+)**: 单线程向内核提交队列 (SQ) 批量发送最多 128 个并发读取操作。实现零系统调用开销的极高 I/O 深度。
    *   **硬链接检测**: 使用并发 `DashMap` 追踪 `(Dev, Inode)`。重复 Inode 只生成元数据条目。

3.  **写入阶段 (Writer, 主线程)**:
    *   从 **Content Channel** 接收数据。
    *   按顺序构建 TAR 流。
    *   流式送入 **并行 Zstd 编码器**（拥有独立的压缩线程池）。

### 2. 解压架构：确定性流水线

解压比压缩更复杂，因为涉及目录时间戳的“竞争条件”。`zstar` 采用 **三阶段屏障 (3-Stage Barrier)** 架构来保证正确性。

1.  **并行提取 (阶段 I)**:
    *   **主线程**: 解析 Tar 流。小文件读入内存发送给 Worker；大文件 (>10MB) 直接流式写入磁盘（防 OOM）。
    *   **Worker**: 并行写入文件。
    *   **优化 - 本地目录缓存**: 每个 Worker 记住已创建的目录，消除 90% 以上的重复 `mkdir` 系统调用。

2.  **硬链接屏障 (阶段 II)**:
    *   硬链接的创建被**推迟**到所有普通文件都写入磁盘之后。这消除了“目标文件尚不存在”的竞争条件。

3.  **元数据恢复 (阶段 III)**:
    *   **目录时间戳悖论**: 在目录中创建文件会更新目录的 `mtime`。
    *   **解决方案**: 所有目录的元数据（权限、时间）都被记录下来，并在最后时刻按 **深度逆序**（最深子目录 -> 根目录）统一应用。

### 3. 核心安全
*   **路径清洗**: 防止 "Zip-Slip" 攻击（绝对路径或 `..` 越权访问）。
*   **跨平台 ACL**: 在 Windows 上模拟近似的 Unix 权限，确保归档跨平台可用。

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
