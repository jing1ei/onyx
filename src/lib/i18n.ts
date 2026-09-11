import { useSyncExternalStore } from 'react';
export type Language = 'zh' | 'en';
let language: Language = typeof localStorage === 'undefined' || localStorage.getItem('onyx.language') === 'en' ? 'en' : 'zh';
const listeners = new Set<() => void>();
export const getLanguage = () => language;
export function setLanguage(next: Language) {
  language = next; localStorage.setItem('onyx.language', next);
  document.documentElement.lang = next === 'zh' ? 'zh-CN' : 'en';
  listeners.forEach(fn => fn());
}
if (typeof window !== 'undefined') window.addEventListener('storage', e => { if (e.key === 'onyx.language') { language = e.newValue === 'en' ? 'en' : 'zh'; document.documentElement.lang = language; listeners.forEach(fn => fn()); } });
export function useLanguage() { return useSyncExternalStore(fn => { listeners.add(fn); return () => { listeners.delete(fn); }; }, getLanguage); }
if (typeof document !== 'undefined') document.documentElement.lang = language === 'zh' ? 'zh-CN' : 'en';

// Source strings are stable keys. File names, paths, units and user theme code
// are deliberately not translated. Both directions share one reviewed table.
const pairs = `Audition|试听
Edit|剪辑
Work mode|工作模式
No audio loaded|未载入音频
Unsaved|未保存
Saved successfully|保存成功
Trim to selection|裁剪到所选
Delete selection|删除所选
Fade in / out|淡入 / 淡出
Fade in|淡入
Fade start|淡变起点
Fade end|淡变终点
Fade curvature|淡变曲率
Curvature|曲率
Linear|线性
Preview curve|试听曲线
Stop preview|停止试听
Apply curve|应用曲线
Cancel curve|取消曲线
Apply or cancel the fade curve first|请先应用或取消淡变曲线
Curve canceled; audio unchanged|已取消曲线，音频未改变
Drag endpoints for range and the middle handle for curvature; Space previews, then apply|拖动两端调整范围，拖动中点调整曲线；空格试听，满意后应用
Fade out|淡出
Fade in applied to selection|已对选区应用淡入
Fade out applied to selection|已对选区应用淡出
Fade from silence to original volume across the selection|选区从静音渐强至原音量，选区长度即淡入时长
Fade from original volume to silence across the selection|选区从原音量渐弱至静音，选区长度即淡出时长
Normalize|标准化音量
Overwrite original|覆盖原文件
Save as|另存为
Discard changes|放弃修改
Undo|撤销
Redo|重做
Zoom|缩放
Zoom out|横向缩小
Zoom in|横向放大
Horizontal zoom|横向缩放倍率
Zoom to selection|放大所选
Fit entire audio|适配全长
Editing tools|编辑工具
Drop audio here, or click to open|把音频拖到这里，或点击打开
Audio waveform, drag to select a region|音频波形，拖动以选择片段
Left channel waveform|左声道波形
Right channel waveform|右声道波形
Go to start|回到开头
Forward five seconds|前进五秒
Play|播放
Pause|暂停
Loop playback|循环播放
Loop|循环
Speed|速度
Volume|音量
Mono|单声道
Stereo|立体声
Open audio to begin|导入一段音频，马上开始
Reading audio…|正在读取音频…
Saving, please wait…|正在保存，请稍候…
Save canceled|已取消保存
Undone|已撤销
Redone|已重做
Restored last saved version|已恢复到上次保存
Trimmed to selection|已裁剪到所选片段
Selection deleted|已删除所选片段
Normalized to -0.4 dB peak|音量已标准化至 -0.4 dB 峰值
Added fade in and out (up to 1.5 seconds)|已添加 1.5 秒淡入淡出
Full audio loaded · Drag to select · R/T zoom · Ctrl+S save|已打开完整音频 · 拖选片段 · R/T 缩放 · Ctrl+S 覆盖保存
Discard unsaved changes?|放弃未保存修改？
Discard unsaved changes and open another file?|当前修改未保存，放弃修改并打开另一个文件？
Please wait for file operations to finish before closing|正在读写文件，请稍候再关闭
Unsaved changes: save or discard before closing|有未保存的修改，请先保存，或点击“放弃修改”
Playlist|播放列表
Nothing queued|暂无音频
Open files|打开文件
Open audio|打开音频
Add|添加
Clear|清空
File|文件
Title|标题
Artist|艺术家
Time|时长
Deck|音轨
Play now|立即播放
Remove|移除
Show in Explorer|在资源管理器中显示
Reveal in Finder|在访达中显示
Assign to deck A|分配到音轨 A
Assign to deck B|分配到音轨 B
Assign this track to a deck|将音频分配到音轨
Add files (⌘⇧O)|添加文件 (Ctrl+Shift+O)
Clear playlist (⌘K)|清空播放列表 (Ctrl+K)
Drop audio files anywhere in the window, or open a folder of masters.|将音频拖入窗口，或打开音频文件夹。
Drop to append|拖入以添加
Drop to play|拖入以播放
drop audio to begin|拖入音频开始
no track loaded|未载入音频
connecting to engine|正在连接音频引擎
audible|正在监听
silent|静音
modified|已修改
Settings|设置
Shortcuts|快捷键
Keyboard|键盘快捷键
Keyboard shortcuts (?)|键盘快捷键 (?)
Esc or ? to close|按 Esc 或 ? 关闭
Settings · appearance, engine source, MIDI bank|设置 · 外观、音频引擎、MIDI 音色库
Close|关闭
Minimise|最小化
Maximise / restore|最大化 / 还原
Maximise or restore|最大化或还原
Previous|上一首
Previous track|上一首
Next|下一首
Next track|下一首
Play / pause (Space)|播放 / 暂停（空格）
Play or pause|播放或暂停
Loop (L) — shift-drag the waveform to set a region|循环 (L) — Shift 拖动波形设置循环区间
Mute|静音
Mute (M)|静音 (M)
Volume (↑ / ↓)|音量 (↑ / ↓)
Monitor|监听
Xfade|交叉淡化
Level-match trim on the audible deck|当前监听音轨的响度匹配增益
Appearance|外观
Theme|主题
Dark|深色
Light|浅色
System default|跟随系统
Accent|强调色
Accent colour, as hex|十六进制强调色
Champagne|香槟金
Bronze|古铜
Terracotta|陶土
Sage|鼠尾草绿
Verdigris|铜绿
Iris|鸢尾紫
Interface font|界面字体
Read-out font|数值字体
System UI|系统界面字体
System mono|系统等宽字体
Grotesk · Helvetica|无衬线 · Helvetica
Humanist · Avenir|人文体 · Avenir
Neutral · Inter|中性体 · Inter
Size|大小
Compact|紧凑
Normal|标准
Large|大号
Reset|重置
Reset appearance|重置外观
How to change the appearance|外观设置方式
Theme code|主题代码
Simple|简单设置
Theme, accent, fonts and size, one control each|分别设置主题、强调色、字体和大小
Hover, pressed and dim states are derived from it|悬停、按下和弱化状态由此颜色自动生成
Monospaced only — LUFS, timecode and dB are tabular|仅等宽字体 — LUFS、时间码与 dB 数值对齐显示
These settings differ from the defaults|这些设置与默认值不同
Back to obsidian, champagne, system fonts, normal size and no theme document|恢复深色、香槟金、系统字体、标准大小并移除自定义主题
Engine source|音频引擎
Audio API|音频接口
Output device|输出设备
Sample rate|采样率
Engine sample rate|引擎采样率
Follow source|跟随源文件
follows deck A · stays bit-transparent|跟随音轨 A · 保持位透明
fixed · resamples|固定采样率 · 重采样
Buffer size|缓冲区大小
Driver default|驱动默认
the driver chooses its own|由驱动决定
Output latency|输出延迟
Output underruns|缓冲欠载次数
no audio API available|没有可用音频接口
no device to ask|没有可查询设备
no output devices on this API|此接口没有输出设备
nothing to enumerate|没有可列出的设备
rebuilding|正在重新初始化
Could not read the audio source:|无法读取音频设备：
General MIDI bank|通用 MIDI 音色库
Choose .sf2|选择 .sf2
Bundled|内置
bundled|内置
bundled with Onyx|Onyx 内置
user bank|自定义音色库
.mid files are rendered through this SoundFont|使用此 SoundFont 渲染 .mid 文件
Loudness cache|响度缓存
Integrated LUFS, LRA and true peak for files you have already played. Waveform peaks are not cached.|缓存已播放文件的综合响度、响度范围和真峰值，不缓存波形。
On disk|磁盘占用
Entries|条目
Clearing|正在清空
Cache unavailable:|缓存不可用：
Every entry is discarded; loudness is re-measured on the next play|清除所有缓存，下次播放时重新测量响度
Read-outs are hidden while a blind test runs: 0.4 LUFS is visible long before it is audible.|盲测期间隐藏测量值，避免视觉数值影响听觉判断。
Theme document|主题文档
The theme document — copy it, hand it to an agent, paste the reply back|主题文档 — 复制、交给助手修改，再粘贴回来
Copy current|复制当前
Copy default|复制默认
Copy for agent|复制给助手
The theme plus a short brief, so pasting it into any chat is enough|复制主题及说明，可直接粘贴给助手
Copy default → paste into a chat → paste the reply back|复制默认主题 → 交给助手修改 → 粘贴修改结果
Open window|独立窗口
Open the editor in its own window, which keeps working if a theme makes this one unreadable|在独立窗口打开主题编辑器，即使当前主题不可读也可继续修改
Apply|应用
Apply to every window|应用到所有窗口
Validate|验证
Check without applying|仅检查，不应用
Fix the errors first|请先修复错误
Go to this line|跳转到此行
Legibility — measured, not blocked. This theme can still be applied.|可读性仅作提示，不阻止应用此主题。
JSON with|支持 JSON，以及
comments and trailing commas. Keys are fixed; unknown ones are errors with a line number.|注释和尾逗号。键名固定，未知键会报告所在行号。
unapplied edits|未应用的修改
more|更多
error|错误
warning|警告
needs|需要
token|主题变量
Equaliser|均衡器
Equaliser (E)|均衡器 (E)
Equaliser (E) · bypassed (⇧E)|均衡器 (E) · 已旁通 (Shift+E)
Equaliser (E) · open in its own window|均衡器 (E) · 在独立窗口中打开
EQ bypass (⇧E)|均衡器旁通 (Shift+E)
Bypassed|已旁通
Engaged|已启用
Bands|频段
Band solo|频段独听
Delete band|删除频段
Delete this band (double-click its node)|删除此频段（双击节点）
Remove every band|删除所有频段
Bypass this band (Alt-click its node)|旁通此频段（Alt 点击节点）
Hold to audition this band's frequency region|按住试听此频段
Filter type|滤波器类型
Cycle the filter type (or right-click the node)|切换滤波器类型（或右键点击节点）
Slope|斜率
Float|置顶
Behind|取消置顶
Floating above other windows · click to let it go behind|当前窗口置顶 · 点击取消置顶
Behind other windows · click to float it on top|当前未置顶 · 点击置顶
Close (E or Esc)|关闭 (E 或 Esc)
No bands. Click anywhere on the curve to add one, or hold|暂无频段，点击曲线添加，或按住
/Ctrl and drag to sweep a solo bandpass across the spectrum.|/Ctrl 拖动以扫听窄带频率。
Click the curve to add a bell, right-click a node for its filter type. Drag a node for frequency and gain, wheel over it for Q, double-click to remove it. Hold|点击曲线添加钟形滤波器，右键选择类型。拖动调整频率与增益，滚轮调整 Q，双击删除。按住
/Ctrl and drag to sweep a solo bandpass. The curve is drawn from the same biquad coefficients the engine runs, not a sketch. Meters stay on the true programme.|/Ctrl 拖动扫听窄带。曲线与引擎使用相同滤波系数，电平表仍显示原节目信号。
A narrow band-pass is being auditioned. This is not the programme. Click to stop.|正在独听窄带，并非完整节目。点击停止。
Bit-transparent|位透明
Bit-transparent means engine rate equals the source rate with EQ bypassed, unity volume and no match trim applied.|位透明表示引擎与源文件采样率一致、均衡器旁通、音量为单位增益且未进行响度匹配。
Polarity inverted|极性反转
Polarity invert — flips the whole deck|极性反转 — 反转整条音轨
Invert the polarity of deck A|反转音轨 A 的极性
Invert the polarity of deck B|反转音轨 B 的极性
Enable A/B comparison|启用 A/B 对比
Align|时间对齐
Auto|自动
B offset|B 偏移
Back to a zero offset|恢复零偏移
Estimate the offset by cross-correlating the two decks|通过两条音轨的互相关估算偏移
Deck B is moved; deck A is the reference timeline|移动音轨 B，音轨 A 为时间参考
Deck B is time-shifted against deck A. Click to reset to zero.|音轨 B 相对 A 存在时间偏移，点击归零。
The best match was polarity-inverted — try ø on one deck.|最佳匹配为极性反转，尝试反转一条音轨的极性。
Keep|保留
Revert|还原
Run again|重新运行
Working|处理中
Blind|盲测
Blind A / B|A / B 盲测
Blind A/B or ABX test|A/B 或 ABX 盲测
ABX test|ABX 盲测
Both decks need a track before a test can start.|两条音轨均需载入音频才能开始盲测。
Finish or abort the blind test first|请先完成或终止盲测
Begin|开始
Abort|终止
Trial|本轮
Trials|轮数
Answer|答案
Confirm|确认
Cancel|取消
Result|结果
Ready|就绪
Complete|已完成
New test|新测试
Running score|当前得分
correct identifications|次正确判断
One-tailed exact binomial|单尾精确二项检验
Final mapping|最终映射
Chose|选择了
hit|正确
miss|错误
None|无
X on the final trial|最后一轮的 X
X = deck|X = 音轨
Y = deck|Y = 音轨
2 slots|2 个位置
3 slots|3 个位置
hidden slot · reference view|隐藏位置 · 参考视图
blind test ·|盲测 ·
· identity hidden|· 身份已隐藏
A fixed reference drawing. It does not follow the audible slot — if it did, switching slots would show you the answer.|固定参考波形，不随监听位置变化，避免切换时泄露答案。
The analyser is masked while a blind test is running: a live spectrum of the audible slot names it as plainly as a meter would.|盲测期间隐藏频谱分析，避免频谱或电平显示泄露答案。
Onyx hit an unexpected error|Onyx 遇到意外错误
Playback is unaffected — the engine runs outside this window. The failure has been written to the log.|播放不受影响，音频引擎独立运行，错误已记录到日志。
Reload the window|重新加载窗口
Designed themes, champagne accent, system fonts — from anywhere, even an unreadable window|恢复默认主题、香槟金及系统字体，即使当前窗口不可读也可使用
Reset appearance|重置外观
Loudness range, LU|响度范围，LU
Corr|相关性
Switch|切换
Click|点击
track|音轨
trial|轮
deck|音轨
inverted|极性反转
trim|增益补偿
switch|切换`;
const enToZh = new Map<string,string>(); const zhToEn = new Map<string,string>();
for (const row of pairs.split('\n')) { const at=row.indexOf('|'); if(at<0) continue; const en=row.slice(0,at),zh=row.slice(at+1); enToZh.set(en,zh); zhToEn.set(zh,en); }
const extra = `tracks|条音轨
Preparing audio, please wait…|正在准备音频，请稍候…
Unable to complete the operation: |无法完成操作：
The editor is not ready yet; please retry|编辑器尚未准备好，请稍后重试
Audio file operations are in progress; please wait before switching|正在读写音频，请稍候再切换
Finish the blind test before editing|请先结束盲测，再进入剪辑
The audible deck is empty; select a loaded A or B deck first|当前监听的音轨没有音频，请先选择已载入音频的 A 或 B
File saved, but audition sync failed: |文件已保存，但试听同步失败：
Audition preparation timed out; please retry|试听准备超时，请重试
Keys: A / B / X to switch, 1 = X is A, 2 = X is B.|按 A / B / X 切换，1 表示 X 是 A，2 表示 X 是 B。
Keys: X / Y to switch, 1 = X is A, 2 = Y is A.|按 X / Y 切换，1 表示 X 是 A，2 表示 Y 是 A。
, or Reset to clear it.|，或点击重置将其清除。
Alt-drag lane B to slide · , / . to nudge · ⇧ = 100 ms, ⌥ = 1 sample|Alt 拖动音轨 B · 逗号 / 句号微调 · Shift = 100 ms，Alt = 1 采样
Onyx (designed)|Onyx 默认设计
in force:|当前应用：
this window is never re-skinned by the document it is editing|本窗口不受正在编辑的主题文档影响
drag = freq / gain · wheel = Q · dbl-click = delete|拖动调整频率 / 增益 · 滚轮调整 Q · 双击删除
⌘/Ctrl-drag = solo sweep|Ctrl 拖动 = 扫频独听
A theme document is in force. It overrides these where they overlap — edit it under|自定义主题正在生效，重叠的设置以主题为准，请在此处编辑：
” is not a colour — use #rrggbb|” 不是有效颜色 — 请使用 #rrggbb
tokens|个主题变量
empty|未载入
MATCHED · pending|已启用匹配 · 等待测量
MATCHED · already level|已匹配 · 响度一致
processed|已处理
bit-transparent|位透明
yes|是
no|否
Level match|响度匹配
Matching…|正在匹配…
Matched|已匹配
Side|侧声道
Swap|左右互换
Side solo|侧声道独听
Left only|仅左声道
Right only|仅右声道
Channels swapped|左右声道互换
Right polarity inverted|右声道极性反转
Play / pause|播放 / 暂停
Zoom out / in|缩小 / 放大
Zoom at pointer|以鼠标位置缩放
Nudge ∓5 s / +5 s (⇧ = 1 s)|后退 / 前进 5 秒（Shift = 1 秒）
Volume ±1 dB|音量 ±1 dB
Seek to start|回到开头
Listen to deck A / deck B · ABX slots A / B|监听音轨 A / B · ABX 位置 A / B
Assign the selected track to deck A / deck B (B turns A/B on)|将所选音频分配到 A / B（B 启用对比）
Toggle A/B deck|切换 A/B 音轨
Blind slot switch (X only in ABX)|切换盲测位置（ABX 仅 X）
Blind vote · A/B: X / Y is deck A · ABX: X = A / X = B|盲测投票 · A/B：X / Y 是 A · ABX：X = A / X = B
Loop on / off|开启 / 关闭循环
EQ window — ⇧E bypasses the EQ|均衡器窗口 — Shift+E 旁通均衡器
EQ band-solo sweep (X = frequency, Y = Q)|均衡器扫频独听（横轴频率，纵轴 Q）
Bypass that EQ band|旁通该均衡频段
Level match on / off (A/B)|开启 / 关闭响度匹配（A/B）
Nudge deck B earlier / later · 10 ms, ⇧ 100 ms, ⌥ 1 sample|前移 / 后移 B · 10 ms，Shift 100 ms，Alt 1 采样
Slide the A/B time offset|拖动 A/B 时间偏移
Remove selected track|移除所选音轨
Open files (replaces playlist)|打开文件（替换列表）
Add files (appends)|添加文件（追加列表）
Clear playlist|清空播放列表
Close the top-most panel|关闭最上层面板
This overlay|显示快捷键
(default)|（默认）
(unavailable)|（不可用）
Drag a track here · tap|拖入音频 · 点击
assigns the selected row|分配所选音频
on a playlist row ·|播放列表中的音频 ·
Monitoring fold only — it sits after the meter tap, so LUFS, true peak and LRA keep describing the true stereo programme, not the fold.|仅影响监听，位于电平测量之后；LUFS、真峰值和响度范围仍反映真实立体声节目。
Currently supports mono or stereo audio|编辑模式目前支持单声道或双声道音频
Source files must be 256 MB or smaller|当前编辑模式支持 256 MB 以内的源文件
Unknown duration or decoded audio exceeds 128 MB|音频时长无法确定或超过当前编辑内存上限（128 MB 解码音频）
Decoded audio exceeds the 256 MiB editing limit; shorten the audio first|解码音频超过 256 MiB 编辑上限，请先缩短音频
Cannot read a valid audio sample rate|无法读取有效的音频采样率
The file changed while opening; please open it again|文件在读取过程中被修改，请重新打开
The original was modified externally; use Save as to avoid overwriting external changes|原文件已被其他程序修改，请另存为，避免覆盖外部修改
The target is read-only; the original was not overwritten|目标文件为只读，未覆盖原文件
The original changed while encoding; it was not overwritten|原文件在编码过程中被修改，未覆盖原文件
Open audio first|请先打开音频
Invalid audio data|无效音频数据
Incomplete audio data|音频数据不完整
Invalid audio format|无效音频格式
Only native-decoded floating-point audio is accepted|仅接受原生解码后的浮点音频`;
for(const row of extra.split('\n')){const [en,zh]=row.split('|');enToZh.set(en,zh);zhToEn.set(zh,en);}
const lower = new Map([...enToZh].map(([en,zh])=>[en.toLowerCase(),zh]));
const longMessages: [string,string][] = [
 ['Match the two decks by integrated LUFS (G).\nOff by default: the signal path is untouched unless you ask for it.\nOnly ever attenuates — the louder deck comes down.','按综合响度匹配两条音轨 (G)。默认关闭，不主动改变信号。只衰减较响的音轨。'],
 ['Paste a theme document here.\n\nCopy default → give it to an agent → paste the reply back.','在此粘贴主题文档。\n\n复制默认主题 → 交给助手修改 → 粘贴修改结果。'],
 ['True peak, dBTP — the label reads CLIP once a sample has clipped.\nClick to reset the peak holds and the clip counter.','真峰值，dBTP — 发生削波时显示 CLIP。\n点击重置峰值保持与削波计数。'],
];
for(const [en,zh] of longMessages){enToZh.set(en,zh);zhToEn.set(zh,en);}
const entities: Record<string,string> = { '&mdash;':'—','&rarr;':'→','&ldquo;':'“','&rdquo;':'”','&nbsp;':' ','&amp;':'&' };
export function t<T>(value: T): T {
  if(typeof value !== 'string') return value;
  const clean=value.replace(/&(?:mdash|rarr|ldquo|rdquo|nbsp|amp);/g,x=>entities[x]);
  const core=clean.trim(); const table=language==='zh'?enToZh:zhToEn;
  const direct=table.get(core) || (language==='zh' ? lower.get(core.toLowerCase()) : undefined);
  if(direct) return clean.replace(core,direct) as T;
  if(language==='zh') {
    const patterns: [RegExp, (...args: string[])=>string][] = [
      [/^(\d+) available$/,(_,n)=>n+' 个可用'],
      [/^(\d+)–(\d+) frames on this device$/,(_,a,b)=>'此设备支持 '+a+'–'+b+' 帧'],
      [/^system default · (.+)$/,(_,s)=>'系统默认 · '+s],
      [/^(\d+) archives$/,(_,n)=>n+' 个压缩包'],
      [/^Rendered through (.+)$/,(_,s)=>'使用音色库 '+s],
      [/^From the archive (.+)$/,(_,s)=>'来自压缩包 '+s],
      [/^Deck B (.+) earlier$/,(_,s)=>'音轨 B 前移 '+s],
      [/^Deck B (.+) later$/,(_,s)=>'音轨 B 后移 '+s],
      [/^Monitor (.+)$/,(_,s)=>'监听 '+t(s)],
      [/^assign to deck (.+)$/i,(_,s)=>'分配到音轨 '+s],
      [/^MATCHED · (.+)$/,(_,s)=>'已匹配 · '+s],
      [/^Monitor: (.+) \((.+)\) — again for stereo$/,(_,s,m)=>'监听：'+t(s)+' ('+m+') — 再按恢复立体声'],
    ];
    for(const [pattern,format] of patterns){const match=core.match(pattern);if(match)return clean.replace(core,format(...match)) as T;}
  }
  // Status messages include a file name: translate only the fixed prefix.
  const prefixes: [string,string][] = [['保存成功：','Saved: '],['保存失败：','Save failed: '],['Could not read the audio source:','无法读取音频设备：']];
  for(const [zh,en] of prefixes){const from=language==='en'?zh:en,to=language==='en'?en:zh;if(clean.startsWith(from))return (to+clean.slice(from.length)) as T;}
  return clean as T;
}
