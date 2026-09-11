# Onyx Branch Windows 版本

这是 MoeCici/Onyx_Branch 的独立修改版，不是 jing1ei/onyx 的官方发布。

支持 Windows 10/11 x64，需要 Microsoft Edge WebView2 Runtime。安装包按当前用户安装，不需要修改音频驱动；便携版解压后运行 Onyx.exe。

安装包和完整便携 ZIP 已附带 FFmpeg 8.0 / FFprobe，安装或完整解压后即可保存音频，无需另外配置 PATH。请保留程序旁的 ffmpeg.exe 和 ffprobe.exe。单独下载 Onyx 主程序 EXE 不含这两个外部工具，优先下载完整便携 ZIP。

FFmpeg 工具遵循 GPLv3，随包提供原始许可、构建说明和 FFmpeg-NOTICE.md；对应 FFmpeg 源码作为同一 Release 的独立附件提供。

程序和安装包未作代码签名。已验证发布构建和无声测试；没有为本次发行运行安装程序或进行真实声卡播放测试。

编辑模式支持单/双声道，源文件及解码浮点数据分别不超过 256 MiB。覆盖前验证原文件未被外部修改；压缩格式保存需要重新编码，不保证保留全部元数据。切换模式不自动保存。

更新前请保存修改并正常关闭旧版本。不要同时运行多个修改版，以免单实例机制将操作转给旧进程。

源码、许可证和更新：https://github.com/MoeCici/Onyx_Branch
