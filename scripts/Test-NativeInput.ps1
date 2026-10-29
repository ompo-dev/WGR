param([Parameter(Mandatory=$true)][int]$GameProcessId,[Parameter(Mandatory=$true)][string]$Report)
$ErrorActionPreference='Stop'
$root=Get-Process -Id $GameProcessId
if($root.ProcessName -ne 'WGR'){throw 'O processo precisa ser o WGR.'}
$ownedWindow=$root.MainWindowHandle
if($ownedWindow -eq [IntPtr]::Zero){throw 'Janela WGR indisponível.'}
Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;
public static class WgrInputSmoke {
 [StructLayout(LayoutKind.Sequential)]public struct Rect{public int Left,Top,Right,Bottom;}
 [StructLayout(LayoutKind.Sequential)]public struct Point{public int X,Y;}
 [StructLayout(LayoutKind.Sequential)]public struct Mouse{public int X,Y;public uint Data,Flags,Time;public UIntPtr Extra;}
 [StructLayout(LayoutKind.Sequential)]public struct Key{public ushort Vk,Scan;public uint Flags,Time;public UIntPtr Extra;}
 [StructLayout(LayoutKind.Explicit)]public struct Data{[FieldOffset(0)]public Mouse Mouse;[FieldOffset(0)]public Key Key;}
 [StructLayout(LayoutKind.Sequential)]public struct Input{public uint Type;public Data Data;}
 [DllImport("user32.dll")]public static extern uint SendInput(uint count,Input[] data,int size);
 [DllImport("user32.dll")]public static extern bool SetForegroundWindow(IntPtr window);
 [DllImport("user32.dll")]public static extern bool AttachThreadInput(uint first,uint second,bool attach);
 [DllImport("kernel32.dll")]public static extern uint GetCurrentThreadId();
 [DllImport("user32.dll")]public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")]public static extern bool GetWindowRect(IntPtr window,out Rect rect);
 [DllImport("user32.dll")]public static extern bool MoveWindow(IntPtr window,int x,int y,int width,int height,bool repaint);
 [DllImport("user32.dll")]public static extern uint MapVirtualKey(uint code,uint type);
 [DllImport("user32.dll")]public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")]public static extern bool GetClipCursor(out Rect rect);
 [DllImport("user32.dll")]public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 public delegate bool EnumChild(IntPtr window,IntPtr state);
 [DllImport("user32.dll")]public static extern bool EnumChildWindows(IntPtr parent,EnumChild callback,IntPtr state);
 [DllImport("user32.dll")]public static extern uint GetWindowThreadProcessId(IntPtr window,out uint process);
 [DllImport("user32.dll")]public static extern bool GetClientRect(IntPtr window,out Rect rect);
 [DllImport("user32.dll")]public static extern bool PostMessage(IntPtr window,uint message,UIntPtr w,IntPtr l);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)]public static extern IntPtr CreateWindowEx(uint ex,string cls,string title,uint style,int x,int y,int w,int h,IntPtr parent,IntPtr menu,IntPtr instance,IntPtr parameter);
 [DllImport("user32.dll")]public static extern bool DestroyWindow(IntPtr window);
 public static void Keyboard(ushort vk,bool up){var v=new Input{Type=1,Data=new Data{Key=new Key{Scan=(ushort)MapVirtualKey(vk,0),Flags=8u+(up?2u:0u)}}};if(SendInput(1,new[]{v},Marshal.SizeOf<Input>())!=1)throw new Exception("Tecla não enviada");}
 public static void Pointer(int x,int y,uint flags){var v=new Input{Type=0,Data=new Data{Mouse=new Mouse{X=x,Y=y,Flags=flags}}};if(SendInput(1,new[]{v},Marshal.SizeOf<Input>())!=1)throw new Exception("Mouse não enviado");}
 public static void ClickChild(IntPtr parent,uint process){IntPtr target=IntPtr.Zero;EnumChildWindows(parent,(window,state)=>{uint owner;GetWindowThreadProcessId(window,out owner);if(owner==process){target=window;return false;}return true;},IntPtr.Zero);if(target==IntPtr.Zero)throw new Exception("HWND nativo ausente");Rect r;GetClientRect(target,out r);var p=new IntPtr((((r.Bottom-r.Top)/2)<<16)|((r.Right-r.Left)/2));PostMessage(target,0x201,new UIntPtr(1),p);PostMessage(target,0x202,UIntPtr.Zero,p);}
 public static void Activate(IntPtr window){var queue=CreateWindowEx(0,"STATIC","WGR input probe",0,0,0,0,0,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero);uint owner;uint foreground=GetWindowThreadProcessId(GetForegroundWindow(),out owner);uint current=GetCurrentThreadId();bool attached=false;try{if(current!=foreground)attached=AttachThreadInput(current,foreground,true);SetForegroundWindow(window);}finally{if(attached)AttachThreadInput(current,foreground,false);if(queue!=IntPtr.Zero)DestroyWindow(queue);}}
}
'@
[void][WgrInputSmoke]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
function Read-State {Get-Content -LiteralPath $Report -Raw | ConvertFrom-Json}
function Wait-State([scriptblock]$Condition) {
 $watch=[Diagnostics.Stopwatch]::StartNew()
 while($watch.Elapsed.TotalSeconds -lt 12){$state=Read-State;if(&$Condition $state){return $state};Start-Sleep -Milliseconds 150}
 throw 'A confirmação de input não chegou em 12 segundos.'
}
function Activate-Game {
 [WgrInputSmoke]::Activate($ownedWindow)
 Start-Sleep -Milliseconds 200
 if([WgrInputSmoke]::GetForegroundWindow() -ne $ownedWindow){throw 'O WGR precisa estar à frente para testar o próprio input.'}
 $rect=[WgrInputSmoke+Rect]::new();[void][WgrInputSmoke]::GetWindowRect($ownedWindow,[ref]$rect)
 [void][WgrInputSmoke]::SetCursorPos(($rect.Left+$rect.Right)/2,($rect.Top+$rect.Bottom)/2)
 Start-Sleep -Milliseconds 150
 $native=[uint32](Read-State).cpu_profiles.native_pid
 $child=Get-CimInstance Win32_Process -Filter "ProcessId=$native"
 if($child.Name -ne 'wgr-rage.exe' -or $child.ParentProcessId -ne $GameProcessId){throw 'Processo nativo fora do WGR.'}
 [WgrInputSmoke]::ClickChild($ownedWindow,$native)
}
$probe=[IntPtr]::Zero
$summary=[ordered]@{backend='native';stage='activate';resize_survives=$false;keyboard_movement=$false;relative_mouse=$false;blur_releases_held_keys=$false;recapture=$false;single_escape_keeps_capture=$false;double_escape_releases_pointer=$false}
try {
 Activate-Game
 $before=Wait-State {param($s) $s.native_input_probe.input.focused -and $s.native_input_probe.input.captured}
 $summary.stage='resize'
 $original=[WgrInputSmoke+Rect]::new();[void][WgrInputSmoke]::GetWindowRect($ownedWindow,[ref]$original)
 $frames=[long]$before.port_frame_report.payload.frames
 foreach($dimensions in @(@(960,640),@(1280,800),@(1050,720),@(($original.Right-$original.Left),($original.Bottom-$original.Top)))){
  [void][WgrInputSmoke]::MoveWindow($ownedWindow,$original.Left,$original.Top,$dimensions[0],$dimensions[1],$true)
  Start-Sleep -Milliseconds 300
 }
 $frames=[long](Read-State).port_frame_report.payload.frames
 $null=Wait-State {param($s) $s.port_frame_report.payload.frames -gt $frames+8 -and $s.port_frame_report.payload.fps -gt 0 -and -not $s.last_notice}
 $childAlive=Get-Process -Id ([int](Read-State).cpu_profiles.native_pid) -ErrorAction SilentlyContinue
 $summary.resize_survives=-not $root.HasExited -and $null -ne $childAlive
 Activate-Game
 $before=Wait-State {param($s) $s.native_input_probe.input.focused -and $s.native_input_probe.input.captured}
 if($before.native_input_probe.input.held_count -ne 0){throw 'Há teclas em uso. O teste não deve disputar o teclado com o jogador.'}
 $events=[long]$before.native_input_probe.input.key_events
 $summary.stage='keyboard'
 [WgrInputSmoke]::Keyboard(0x57,$false);Start-Sleep -Milliseconds 2200;[WgrInputSmoke]::Keyboard(0x57,$true)
 $after=Wait-State {param($s) $s.native_input_probe.input.key_events -ge $events+2 -and $s.native_input_probe.input.held_count -eq 0}
 $distance=0.0;for($i=0;$i -lt 3;$i++){$distance+=[math]::Pow(($after.native_input_probe.position[$i]-$before.native_input_probe.position[$i]),2)}
 $summary.moved_metres=[math]::Sqrt($distance)
 $summary.keyboard_movement=$summary.moved_metres -gt 0.15
 $mouse=[long]$after.native_input_probe.input.relative_mouse_events
 $summary.stage='relative_mouse'
 [WgrInputSmoke]::Pointer(36,-8,0x2001)
 Start-Sleep -Milliseconds 1800
 $afterMouse=Read-State
 $summary.relative_mouse=([long]$afterMouse.port_frame_report.payload.input.relative_mouse_events -gt $mouse)
 # Hold a key, then move focus to this test's own native window before key-up.
 $summary.stage='blur'
 [WgrInputSmoke]::Keyboard(0x57,$false);Start-Sleep -Milliseconds 250
 $probe=[WgrInputSmoke]::CreateWindowEx(0,'STATIC','WGR — verificação de foco',0x10CF0000,40,40,320,120,[IntPtr]::Zero,[IntPtr]::Zero,[IntPtr]::Zero,[IntPtr]::Zero)
 if($probe -eq [IntPtr]::Zero){throw 'Não foi possível criar a janela do teste.'}
 [WgrInputSmoke]::Activate($probe)
 $blur=Wait-State {param($s) -not $s.port_frame_report.payload.input.focused -and -not $s.port_frame_report.payload.input.captured -and $s.port_frame_report.payload.input.held_count -eq 0}
 $summary.blur_releases_held_keys=$true
 [WgrInputSmoke]::Keyboard(0x57,$true)
 [void][WgrInputSmoke]::DestroyWindow($probe);$probe=[IntPtr]::Zero
 Activate-Game
 $summary.stage='recapture'
 $null=Wait-State {param($s) $s.port_frame_report.payload.input.focused -and $s.port_frame_report.payload.input.captured}
 $summary.recapture=$true
 $summary.stage='escape'
 [WgrInputSmoke]::Keyboard(0x1B,$false);[WgrInputSmoke]::Keyboard(0x1B,$true)
 Start-Sleep -Milliseconds 1500
 $summary.single_escape_keeps_capture=(Read-State).port_frame_report.payload.input.captured -eq $true
 [WgrInputSmoke]::Keyboard(0x1B,$false);[WgrInputSmoke]::Keyboard(0x1B,$true)
 Start-Sleep -Milliseconds 120
 [WgrInputSmoke]::Keyboard(0x1B,$false);[WgrInputSmoke]::Keyboard(0x1B,$true)
 $null=Wait-State {param($s) -not $s.port_frame_report.payload.input.captured}
 $clip=[WgrInputSmoke+Rect]::new();[void][WgrInputSmoke]::GetClipCursor([ref]$clip)
 $summary.double_escape_releases_pointer=($clip.Right-$clip.Left -gt 1 -and $clip.Bottom-$clip.Top -gt 1)
 $summary.passed=$summary.resize_survives -and $summary.keyboard_movement -and $summary.relative_mouse -and $summary.blur_releases_held_keys -and $summary.recapture -and $summary.single_escape_keeps_capture -and $summary.double_escape_releases_pointer
 $summary.stage='complete'
} finally {
 [WgrInputSmoke]::Keyboard(0x57,$true)
 if($probe -ne [IntPtr]::Zero){[void][WgrInputSmoke]::DestroyWindow($probe)}
 $destination=Join-Path (Split-Path $Report) 'native-input-smoke.json'
 $summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $destination
}
$summary | ConvertTo-Json -Depth 5
if([WgrInputSmoke]::GetForegroundWindow() -eq $ownedWindow){
 Add-Type -AssemblyName System.Drawing
 $rect=[WgrInputSmoke+Rect]::new();[void][WgrInputSmoke]::GetWindowRect($ownedWindow,[ref]$rect)
 $bitmap=[Drawing.Bitmap]::new(($rect.Right-$rect.Left),($rect.Bottom-$rect.Top))
 $graphics=[Drawing.Graphics]::FromImage($bitmap)
 try{$graphics.CopyFromScreen($rect.Left,$rect.Top,0,0,$bitmap.Size);$bitmap.Save((Join-Path (Split-Path $Report) 'native-resize-input-frame.png'),[Drawing.Imaging.ImageFormat]::Png)}finally{$graphics.Dispose();$bitmap.Dispose()}
}
if(-not $summary.passed){throw 'O input nativo ainda não passou todos os controles.'}
