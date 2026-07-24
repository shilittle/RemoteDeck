[CmdletBinding()]
param(
    [switch]$SelfTest,
    [switch]$NoAutoStart
)

$ErrorActionPreference = 'Stop'
$script:AppRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$script:ConfigPath = Join-Path $script:AppRoot 'config.json'
$script:TelemetryScriptPath = Join-Path $script:AppRoot 'remote_telemetry.py'
$script:LogDirectory = Join-Path $script:AppRoot 'logs'
$script:SshExe = Join-Path $env:WINDIR 'System32\OpenSSH\ssh.exe'

function Read-AppConfig {
    if (-not (Test-Path -LiteralPath $script:ConfigPath)) {
        throw "找不到配置文件：$script:ConfigPath"
    }
    $config = Get-Content -Raw -Encoding UTF8 -LiteralPath $script:ConfigPath | ConvertFrom-Json
    foreach ($name in @([string]$config.sshHost, [string]$config.forwardHost)) {
        if ($name -notmatch '^[A-Za-z0-9_.-]+$') {
            throw "SSH 别名包含不安全字符：$name"
        }
    }
    return $config
}

$script:Config = Read-AppConfig

function Resolve-SshTarget {
    param([Parameter(Mandatory = $true)][string]$Alias)
    $result = [ordered]@{ HostName = ''; User = ''; Port = 22 }
    $lines = & $script:SshExe -G $Alias 2>$null
    foreach ($line in $lines) {
        if ($line -match '^hostname\s+(.+)$') { $result.HostName = $Matches[1].Trim() }
        elseif ($line -match '^user\s+(.+)$') { $result.User = $Matches[1].Trim() }
        elseif ($line -match '^port\s+(\d+)$') { $result.Port = [int]$Matches[1] }
    }
    return [pscustomobject]$result
}

function Test-TcpEndpoint {
    param(
        [Parameter(Mandatory = $true)][string]$Address,
        [Parameter(Mandatory = $true)][int]$Port,
        [int]$TimeoutMilliseconds = 1200
    )
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $async = $client.BeginConnect($Address, $Port, $null, $null)
        if (-not $async.AsyncWaitHandle.WaitOne($TimeoutMilliseconds, $false)) {
            return $false
        }
        $client.EndConnect($async)
        return $client.Connected
    }
    catch {
        return $false
    }
    finally {
        $client.Close()
    }
}

function Invoke-SelfTest {
    $target = Resolve-SshTarget -Alias ([string]$script:Config.sshHost)
    $tcp = Test-TcpEndpoint -Address $target.HostName -Port $target.Port -TimeoutMilliseconds 2500
    $remoteOutput = & $script:SshExe -o BatchMode=yes -o ConnectTimeout=15 -o ClearAllForwardings=yes $script:Config.sshHost 'command -v btop; btop --version; command -v python3; printf SELF_TEST_OK' 2>&1
    $remoteExit = $LASTEXITCODE
    $forwardConfig = & $script:SshExe -G $script:Config.forwardHost 2>$null |
        Select-String -Pattern '^remoteforward\s+'
    $result = [ordered]@{
        config                 = $script:ConfigPath
        sshExecutable          = $script:SshExe
        sshAlias               = [string]$script:Config.sshHost
        resolvedEndpoint       = "$($target.HostName):$($target.Port)"
        endpointReachable      = $tcp
        remoteProbeExitCode    = $remoteExit
        remoteProbeOutput      = ($remoteOutput -join "`n")
        telemetryScriptPresent = Test-Path -LiteralPath $script:TelemetryScriptPath
        remoteForwardConfigured = $null -ne $forwardConfig
        success                = $tcp -and ($remoteExit -eq 0) -and (Test-Path -LiteralPath $script:TelemetryScriptPath) -and ($null -ne $forwardConfig)
    }
    $result | ConvertTo-Json -Depth 5
    if (-not $result.success) { exit 1 }
    exit 0
}

if ($SelfTest) {
    Invoke-SelfTest
}

Add-Type -AssemblyName PresentationFramework
Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName WindowsBase
Add-Type -AssemblyName Microsoft.VisualBasic

if (-not (Test-Path -LiteralPath $script:LogDirectory)) {
    [void](New-Item -ItemType Directory -Path $script:LogDirectory -Force)
}
$script:LogPath = Join-Path $script:LogDirectory ("monitor-{0}.log" -f (Get-Date -Format 'yyyyMMdd'))

function Write-AppLog {
    param(
        [Parameter(Mandatory = $true)][string]$Message,
        [ValidateSet('INFO', 'WARN', 'ERROR')][string]$Level = 'INFO'
    )
    $line = "{0} [{1}] {2}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Level, $Message
    try { Add-Content -LiteralPath $script:LogPath -Value $line -Encoding UTF8 } catch {}
    if ($script:LogBox) {
        $script:LogBox.AppendText($line + "`r`n")
        if ($script:LogBox.Text.Length -gt 180000) {
            $script:LogBox.Text = $script:LogBox.Text.Substring($script:LogBox.Text.Length - 120000)
        }
        $script:LogBox.ScrollToEnd()
    }
}

function Format-Bytes {
    param([double]$Value)
    if ($Value -ge 1TB) { return "{0:N1} TiB" -f ($Value / 1TB) }
    if ($Value -ge 1GB) { return "{0:N1} GiB" -f ($Value / 1GB) }
    if ($Value -ge 1MB) { return "{0:N1} MiB" -f ($Value / 1MB) }
    if ($Value -ge 1KB) { return "{0:N1} KiB" -f ($Value / 1KB) }
    return "{0:N0} B" -f $Value
}

function Format-Rate {
    param([double]$Value)
    return "$(Format-Bytes $Value)/s"
}

function Format-Uptime {
    param([int64]$Seconds)
    $span = [TimeSpan]::FromSeconds($Seconds)
    if ($span.Days -gt 0) { return "{0} 天 {1:00}:{2:00}:{3:00}" -f $span.Days, $span.Hours, $span.Minutes, $span.Seconds }
    return "{0:00}:{1:00}:{2:00}" -f $span.Hours, $span.Minutes, $span.Seconds
}

function Set-StatusChip {
    param(
        [System.Windows.Controls.TextBlock]$Control,
        [string]$Text,
        [ValidateSet('good', 'warn', 'bad', 'idle')][string]$State = 'idle'
    )
    $colors = @{
        good = '#4ADE80'
        warn = '#FBBF24'
        bad  = '#FB7185'
        idle = '#94A3B8'
    }
    $Control.Text = $Text
    $Control.Foreground = [System.Windows.Media.BrushConverter]::new().ConvertFromString($colors[$State])
}

$xaml = @'
<Window xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        Title="LabPulse SSH" Width="1420" Height="920" MinWidth="1120" MinHeight="720"
        WindowStartupLocation="CenterScreen" Background="#08111F" Foreground="#E5EDF7"
        FontFamily="Segoe UI">
    <Window.Resources>
        <SolidColorBrush x:Key="PanelBrush" Color="#111D2E"/>
        <SolidColorBrush x:Key="PanelAltBrush" Color="#16243A"/>
        <SolidColorBrush x:Key="BorderBrush" Color="#263854"/>
        <SolidColorBrush x:Key="AccentBrush" Color="#38BDF8"/>
        <Style TargetType="Button">
            <Setter Property="Background" Value="#1F3652"/>
            <Setter Property="Foreground" Value="#E8F4FF"/>
            <Setter Property="BorderBrush" Value="#365577"/>
            <Setter Property="BorderThickness" Value="1"/>
            <Setter Property="Padding" Value="14,8"/>
            <Setter Property="Margin" Value="4"/>
            <Setter Property="Cursor" Value="Hand"/>
            <Setter Property="FontWeight" Value="SemiBold"/>
            <Style.Triggers>
                <Trigger Property="IsMouseOver" Value="True"><Setter Property="Background" Value="#285078"/></Trigger>
                <Trigger Property="IsEnabled" Value="False"><Setter Property="Opacity" Value="0.45"/></Trigger>
            </Style.Triggers>
        </Style>
        <Style TargetType="TextBox">
            <Setter Property="Background" Value="#0B1627"/>
            <Setter Property="Foreground" Value="#DCEBFA"/>
            <Setter Property="BorderBrush" Value="#2A415F"/>
            <Setter Property="CaretBrush" Value="#7DD3FC"/>
            <Setter Property="Padding" Value="8"/>
        </Style>
        <Style TargetType="CheckBox">
            <Setter Property="Foreground" Value="#D8E6F4"/>
            <Setter Property="Margin" Value="8,4"/>
        </Style>
        <Style TargetType="ProgressBar">
            <Setter Property="Height" Value="8"/>
            <Setter Property="Foreground" Value="#38BDF8"/>
            <Setter Property="Background" Value="#24364D"/>
            <Setter Property="BorderThickness" Value="0"/>
        </Style>
        <Style TargetType="TabItem">
            <Setter Property="Foreground" Value="#B8C9DB"/>
            <Setter Property="Background" Value="#0C1728"/>
            <Setter Property="Padding" Value="18,10"/>
            <Setter Property="FontWeight" Value="SemiBold"/>
            <Style.Triggers>
                <Trigger Property="IsSelected" Value="True">
                    <Setter Property="Foreground" Value="#7DD3FC"/>
                    <Setter Property="Background" Value="#16243A"/>
                </Trigger>
            </Style.Triggers>
        </Style>
        <Style TargetType="DataGrid">
            <Setter Property="Background" Value="#0B1627"/>
            <Setter Property="Foreground" Value="#DCEBFA"/>
            <Setter Property="BorderBrush" Value="#263854"/>
            <Setter Property="RowBackground" Value="#0D192B"/>
            <Setter Property="AlternatingRowBackground" Value="#111F33"/>
            <Setter Property="GridLinesVisibility" Value="Horizontal"/>
            <Setter Property="HorizontalGridLinesBrush" Value="#1E3048"/>
            <Setter Property="HeadersVisibility" Value="Column"/>
            <Setter Property="AutoGenerateColumns" Value="False"/>
            <Setter Property="IsReadOnly" Value="True"/>
        </Style>
        <Style TargetType="DataGridColumnHeader">
            <Setter Property="Background" Value="#182942"/>
            <Setter Property="Foreground" Value="#AFC5D9"/>
            <Setter Property="BorderBrush" Value="#29415F"/>
            <Setter Property="Padding" Value="8"/>
            <Setter Property="FontWeight" Value="SemiBold"/>
        </Style>
        <Style x:Key="CardBorder" TargetType="Border">
            <Setter Property="Background" Value="{StaticResource PanelBrush}"/>
            <Setter Property="BorderBrush" Value="{StaticResource BorderBrush}"/>
            <Setter Property="BorderThickness" Value="1"/>
            <Setter Property="CornerRadius" Value="10"/>
            <Setter Property="Padding" Value="16"/>
            <Setter Property="Margin" Value="6"/>
        </Style>
    </Window.Resources>

    <Grid Margin="12">
        <Grid.RowDefinitions>
            <RowDefinition Height="Auto"/>
            <RowDefinition Height="Auto"/>
            <RowDefinition Height="*"/>
            <RowDefinition Height="Auto"/>
        </Grid.RowDefinitions>

        <Grid Grid.Row="0" Margin="4,2,4,10">
            <Grid.ColumnDefinitions>
                <ColumnDefinition Width="*"/>
                <ColumnDefinition Width="Auto"/>
            </Grid.ColumnDefinitions>
            <StackPanel>
                <TextBlock Text="LabPulse SSH" FontSize="26" FontWeight="Bold" Foreground="#F1F7FD"/>
                <TextBlock Text="SSH · btop 保活 · 性能遥测 · 安全命令 · 远程端口转发" Foreground="#8298AE" Margin="0,3,0,0"/>
            </StackPanel>
            <Button Grid.Column="1" x:Name="RefreshAllButton" Content="立即检查" Padding="18,9"/>
        </Grid>

        <Border Grid.Row="1" Background="#0E1B2D" BorderBrush="#263854" BorderThickness="1" CornerRadius="10" Padding="12" Margin="4,0,4,10">
            <UniformGrid Columns="4">
                <StackPanel Margin="10,2"><TextBlock Text="SSH 端口" Foreground="#7F95AA"/><TextBlock x:Name="SshStatusText" Text="检查中…" FontSize="15" FontWeight="SemiBold"/></StackPanel>
                <StackPanel Margin="10,2"><TextBlock Text="性能遥测" Foreground="#7F95AA"/><TextBlock x:Name="TelemetryStatusText" Text="未启动" FontSize="15" FontWeight="SemiBold"/></StackPanel>
                <StackPanel Margin="10,2"><TextBlock Text="btop 保活" Foreground="#7F95AA"/><TextBlock x:Name="BtopStatusText" Text="未启动" FontSize="15" FontWeight="SemiBold"/></StackPanel>
                <StackPanel Margin="10,2"><TextBlock Text="反向端口转发" Foreground="#7F95AA"/><TextBlock x:Name="ForwardStatusText" Text="未启动" FontSize="15" FontWeight="SemiBold"/></StackPanel>
            </UniformGrid>
        </Border>

        <TabControl Grid.Row="2" Background="#08111F" BorderThickness="0">
            <TabItem Header="性能总览">
                <Grid Margin="4,10,4,4">
                    <Grid.RowDefinitions>
                        <RowDefinition Height="Auto"/>
                        <RowDefinition Height="230"/>
                        <RowDefinition Height="*"/>
                    </Grid.RowDefinitions>
                    <UniformGrid Grid.Row="0" Columns="4">
                        <Border Style="{StaticResource CardBorder}">
                            <StackPanel><TextBlock Text="CPU" Foreground="#8298AE"/><TextBlock x:Name="CpuValueText" Text="-- %" FontSize="28" FontWeight="Bold" Margin="0,4"/><ProgressBar x:Name="CpuProgress" Value="0"/><TextBlock x:Name="CpuDetailText" Text="等待数据" Foreground="#8FA6BC" Margin="0,7,0,0"/></StackPanel>
                        </Border>
                        <Border Style="{StaticResource CardBorder}">
                            <StackPanel><TextBlock Text="内存" Foreground="#8298AE"/><TextBlock x:Name="MemoryValueText" Text="-- %" FontSize="28" FontWeight="Bold" Margin="0,4"/><ProgressBar x:Name="MemoryProgress" Value="0" Foreground="#A78BFA"/><TextBlock x:Name="MemoryDetailText" Text="等待数据" Foreground="#8FA6BC" Margin="0,7,0,0"/></StackPanel>
                        </Border>
                        <Border Style="{StaticResource CardBorder}">
                            <StackPanel><TextBlock Text="系统负载" Foreground="#8298AE"/><TextBlock x:Name="LoadValueText" Text="--" FontSize="28" FontWeight="Bold" Margin="0,4"/><TextBlock x:Name="LoadDetailText" Text="1 / 5 / 15 分钟" Foreground="#8FA6BC" Margin="0,15,0,0"/></StackPanel>
                        </Border>
                        <Border Style="{StaticResource CardBorder}">
                            <StackPanel><TextBlock Text="运行时间" Foreground="#8298AE"/><TextBlock x:Name="UptimeValueText" Text="--" FontSize="24" FontWeight="Bold" Margin="0,8"/><TextBlock x:Name="HostValueText" Text="等待连接" Foreground="#8FA6BC" Margin="0,13,0,0"/></StackPanel>
                        </Border>
                    </UniformGrid>

                    <Grid Grid.Row="1">
                        <Grid.ColumnDefinitions><ColumnDefinition/><ColumnDefinition/><ColumnDefinition/></Grid.ColumnDefinitions>
                        <Border Grid.Column="0" Style="{StaticResource CardBorder}">
                            <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions><TextBlock Text="CPU 历史" FontWeight="SemiBold"/><Canvas x:Name="CpuChart" Grid.Row="1" Margin="0,10,0,0" ClipToBounds="True"/></Grid>
                        </Border>
                        <Border Grid.Column="1" Style="{StaticResource CardBorder}">
                            <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions><TextBlock Text="内存历史" FontWeight="SemiBold"/><Canvas x:Name="MemoryChart" Grid.Row="1" Margin="0,10,0,0" ClipToBounds="True"/></Grid>
                        </Border>
                        <Border Grid.Column="2" Style="{StaticResource CardBorder}">
                            <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions><TextBlock x:Name="NetworkTitleText" Text="网络吞吐" FontWeight="SemiBold"/><Canvas x:Name="NetworkChart" Grid.Row="1" Margin="0,10,0,0" ClipToBounds="True"/></Grid>
                        </Border>
                    </Grid>

                    <Grid Grid.Row="2">
                        <Grid.ColumnDefinitions><ColumnDefinition Width="2.1*"/><ColumnDefinition Width="1*"/></Grid.ColumnDefinitions>
                        <Border Grid.Column="0" Style="{StaticResource CardBorder}">
                            <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                                <TextBlock Text="高 CPU 进程" FontSize="15" FontWeight="SemiBold"/>
                                <DataGrid x:Name="ProcessGrid" Grid.Row="1" Margin="0,10,0,0" AlternationCount="2">
                                    <DataGrid.Columns>
                                        <DataGridTextColumn Header="PID" Binding="{Binding pid}" Width="70"/>
                                        <DataGridTextColumn Header="用户" Binding="{Binding user}" Width="100"/>
                                        <DataGridTextColumn Header="CPU %" Binding="{Binding cpu}" Width="75"/>
                                        <DataGridTextColumn Header="内存 %" Binding="{Binding memory}" Width="75"/>
                                        <DataGridTextColumn Header="状态" Binding="{Binding state}" Width="65"/>
                                        <DataGridTextColumn Header="已运行" Binding="{Binding elapsed}" Width="95"/>
                                        <DataGridTextColumn Header="命令" Binding="{Binding command}" Width="*"/>
                                    </DataGrid.Columns>
                                </DataGrid>
                            </Grid>
                        </Border>
                        <ScrollViewer Grid.Column="1" VerticalScrollBarVisibility="Auto">
                            <StackPanel>
                                <Border Style="{StaticResource CardBorder}">
                                    <StackPanel><TextBlock Text="GPU" FontSize="15" FontWeight="SemiBold"/><StackPanel x:Name="GpuPanel" Margin="0,8,0,0"><TextBlock Text="等待数据" Foreground="#8298AE"/></StackPanel></StackPanel>
                                </Border>
                                <Border Style="{StaticResource CardBorder}">
                                    <StackPanel><TextBlock Text="磁盘" FontSize="15" FontWeight="SemiBold"/><StackPanel x:Name="DiskPanel" Margin="0,8,0,0"><TextBlock Text="等待数据" Foreground="#8298AE"/></StackPanel></StackPanel>
                                </Border>
                            </StackPanel>
                        </ScrollViewer>
                    </Grid>
                </Grid>
            </TabItem>

            <TabItem Header="btop 保活">
                <Grid Margin="10">
                    <Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                    <Border Style="{StaticResource CardBorder}">
                        <StackPanel>
                            <TextBlock Text="btop 会话看门狗" FontSize="20" FontWeight="Bold"/>
                            <TextBlock TextWrapping="Wrap" Foreground="#91A6BA" Margin="0,8,0,0"
                                       Text="btop 1.3.0 没有机器可读输出。本程序保活真实 btop TTY 会话；图形性能面板读取 btop 同源的 /proc、ps、df 与 nvidia-smi。隐藏会话退出会自动重启，并按周期主动轮换，避免长时间假死。"/>
                            <TextBlock x:Name="BtopDetailText" Text="尚未启动" Margin="0,12,0,0" Foreground="#CBD8E5"/>
                        </StackPanel>
                    </Border>
                    <StackPanel Grid.Row="1" Orientation="Horizontal" Margin="6">
                        <Button x:Name="BtopStartButton" Content="启动保活"/>
                        <Button x:Name="BtopRestartButton" Content="重启 btop"/>
                        <Button x:Name="BtopInteractiveButton" Content="打开交互终端"/>
                        <Button x:Name="BtopStopButton" Content="停止"/>
                        <CheckBox x:Name="BtopAutoRestartCheck" Content="意外退出自动重启" IsChecked="True" VerticalAlignment="Center"/>
                    </StackPanel>
                    <Border Grid.Row="2" Style="{StaticResource CardBorder}">
                        <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                            <TextBlock Text="btop 事件" FontWeight="SemiBold"/>
                            <TextBox x:Name="BtopLogBox" Grid.Row="1" Margin="0,10,0,0" IsReadOnly="True" AcceptsReturn="True" TextWrapping="Wrap" VerticalScrollBarVisibility="Auto" FontFamily="Consolas"/>
                        </Grid>
                    </Border>
                </Grid>
            </TabItem>

            <TabItem Header="快捷命令">
                <Grid Margin="10">
                    <Grid.ColumnDefinitions><ColumnDefinition Width="330"/><ColumnDefinition Width="*"/></Grid.ColumnDefinitions>
                    <Border Grid.Column="0" Style="{StaticResource CardBorder}">
                        <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/><RowDefinition Height="Auto"/></Grid.RowDefinitions>
                            <TextBlock Text="命令预设" FontSize="18" FontWeight="Bold"/>
                            <ListBox x:Name="PresetList" Grid.Row="1" Margin="0,10" Background="#0B1627" Foreground="#DCEBFA" BorderBrush="#2A415F" DisplayMemberPath="name"/>
                            <Button x:Name="RunPresetButton" Grid.Row="2" Content="执行所选预设"/>
                        </Grid>
                    </Border>
                    <Grid Grid.Column="1">
                        <Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                        <Border Style="{StaticResource CardBorder}">
                            <StackPanel><TextBlock x:Name="PresetNameText" Text="选择一个预设" FontSize="18" FontWeight="Bold"/><TextBlock x:Name="PresetRiskText" Text="" Foreground="#FBBF24" Margin="0,5,0,0"/><TextBlock x:Name="PresetDescriptionText" TextWrapping="Wrap" Foreground="#93A8BC" Margin="0,7,0,0"/></StackPanel>
                        </Border>
                        <Border Grid.Row="1" Style="{StaticResource CardBorder}">
                            <Grid><Grid.ColumnDefinitions><ColumnDefinition Width="*"/><ColumnDefinition Width="Auto"/></Grid.ColumnDefinitions>
                                <StackPanel><TextBlock Text="自定义命令" FontWeight="SemiBold"/><TextBox x:Name="CustomCommandBox" Margin="0,8,0,0" MinHeight="58" AcceptsReturn="True" FontFamily="Consolas" TextWrapping="Wrap"/></StackPanel>
                                <Button x:Name="RunCustomButton" Grid.Column="1" Content="发送命令" VerticalAlignment="Bottom" Margin="12,4,4,4"/>
                            </Grid>
                        </Border>
                        <Border Grid.Row="2" Style="{StaticResource CardBorder}">
                            <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                                <DockPanel><TextBlock Text="命令输出" FontWeight="SemiBold"/><Button x:Name="ClearCommandOutputButton" Content="清空" DockPanel.Dock="Right" Padding="10,4"/></DockPanel>
                                <TextBox x:Name="CommandOutputBox" Grid.Row="1" Margin="0,10,0,0" IsReadOnly="True" AcceptsReturn="True" TextWrapping="NoWrap" HorizontalScrollBarVisibility="Auto" VerticalScrollBarVisibility="Auto" FontFamily="Consolas"/>
                            </Grid>
                        </Border>
                    </Grid>
                </Grid>
            </TabItem>

            <TabItem Header="端口转发">
                <Grid Margin="10">
                    <Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                    <UniformGrid Grid.Row="0" Columns="3">
                        <Border Style="{StaticResource CardBorder}"><StackPanel><TextBlock Text="SSH 服务端点" Foreground="#8298AE"/><TextBlock x:Name="EndpointDetailText" Text="检查中…" FontSize="18" FontWeight="SemiBold" Margin="0,7,0,0"/></StackPanel></Border>
                        <Border Style="{StaticResource CardBorder}"><StackPanel><TextBlock Text="远端监听" Foreground="#8298AE"/><TextBlock x:Name="RemoteForwardDetailText" Text="127.0.0.1:17890" FontSize="18" FontWeight="SemiBold" Margin="0,7,0,0"/></StackPanel></Border>
                        <Border Style="{StaticResource CardBorder}"><StackPanel><TextBlock Text="本地目标" Foreground="#8298AE"/><TextBlock x:Name="LocalTargetDetailText" Text="127.0.0.1:7890" FontSize="18" FontWeight="SemiBold" Margin="0,7,0,0"/></StackPanel></Border>
                    </UniformGrid>
                    <StackPanel Grid.Row="1" Orientation="Horizontal" Margin="6">
                        <Button x:Name="ForwardStartButton" Content="启动转发"/>
                        <Button x:Name="ForwardTestButton" Content="立即测试"/>
                        <Button x:Name="ForwardRestartButton" Content="释放并重启"/>
                        <Button x:Name="ForwardStopButton" Content="停止"/>
                        <Button x:Name="ForwardReleaseButton" Content="仅释放远端端口"/>
                        <CheckBox x:Name="ForwardAutoRestartCheck" Content="断线自动释放并重连" IsChecked="True" VerticalAlignment="Center"/>
                    </StackPanel>
                    <Border Grid.Row="2" Style="{StaticResource CardBorder}">
                        <Grid><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                            <TextBlock Text="转发事件" FontWeight="SemiBold"/>
                            <TextBox x:Name="ForwardLogBox" Grid.Row="1" Margin="0,10,0,0" IsReadOnly="True" AcceptsReturn="True" TextWrapping="Wrap" VerticalScrollBarVisibility="Auto" FontFamily="Consolas"/>
                        </Grid>
                    </Border>
                </Grid>
            </TabItem>

            <TabItem Header="日志">
                <Grid Margin="10">
                    <Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
                    <DockPanel><TextBlock x:Name="LogPathText" FontSize="14" Foreground="#91A6BA"/><Button x:Name="ClearLogViewButton" Content="清空显示" DockPanel.Dock="Right" Padding="10,4"/></DockPanel>
                    <TextBox x:Name="LogBox" Grid.Row="1" Margin="0,10,0,0" IsReadOnly="True" AcceptsReturn="True" TextWrapping="Wrap" VerticalScrollBarVisibility="Auto" FontFamily="Consolas"/>
                </Grid>
            </TabItem>
        </TabControl>

        <Grid Grid.Row="3" Margin="6,8,6,0">
            <Grid.ColumnDefinitions><ColumnDefinition Width="*"/><ColumnDefinition Width="Auto"/></Grid.ColumnDefinitions>
            <TextBlock x:Name="FooterStatusText" Text="正在初始化…" Foreground="#8298AE"/>
            <TextBlock Grid.Column="1" Text="关闭窗口会安全停止本程序创建的 SSH 与 btop 会话" Foreground="#5F7489"/>
        </Grid>
    </Grid>
</Window>
'@

$reader = New-Object System.Xml.XmlNodeReader ([xml]$xaml)
$script:Window = [Windows.Markup.XamlReader]::Load($reader)

$controlNames = @(
    'RefreshAllButton', 'SshStatusText', 'TelemetryStatusText', 'BtopStatusText', 'ForwardStatusText',
    'CpuValueText', 'CpuProgress', 'CpuDetailText', 'MemoryValueText', 'MemoryProgress', 'MemoryDetailText',
    'LoadValueText', 'LoadDetailText', 'UptimeValueText', 'HostValueText', 'CpuChart', 'MemoryChart',
    'NetworkChart', 'NetworkTitleText', 'ProcessGrid', 'GpuPanel', 'DiskPanel',
    'BtopDetailText', 'BtopStartButton', 'BtopRestartButton', 'BtopInteractiveButton', 'BtopStopButton',
    'BtopAutoRestartCheck', 'BtopLogBox', 'PresetList', 'RunPresetButton', 'PresetNameText',
    'PresetRiskText', 'PresetDescriptionText', 'CustomCommandBox', 'RunCustomButton',
    'ClearCommandOutputButton', 'CommandOutputBox', 'EndpointDetailText', 'RemoteForwardDetailText',
    'LocalTargetDetailText', 'ForwardStartButton', 'ForwardTestButton', 'ForwardRestartButton',
    'ForwardStopButton', 'ForwardReleaseButton', 'ForwardAutoRestartCheck', 'ForwardLogBox',
    'LogPathText', 'ClearLogViewButton', 'LogBox', 'FooterStatusText'
)
foreach ($name in $controlNames) {
    Set-Variable -Scope Script -Name $name -Value $script:Window.FindName($name)
}

$script:LogPathText.Text = "日志文件：$script:LogPath"
$script:PresetList.ItemsSource = $script:Config.presets
$script:BtopAutoRestartCheck.IsChecked = [bool]$script:Config.btop.autoRestart
$script:ForwardAutoRestartCheck.IsChecked = [bool]$script:Config.forward.autoRestart
$script:RemoteForwardDetailText.Text = "{0}:{1}" -f $script:Config.forward.remoteBindAddress, $script:Config.forward.remotePort
$script:LocalTargetDetailText.Text = "{0}:{1}" -f $script:Config.forward.localTargetAddress, $script:Config.forward.localTargetPort

$script:ShuttingDown = $false
$script:Telemetry = $null
$script:TelemetryLastData = $null
$script:TelemetryLastStart = [DateTime]::MinValue
$script:TelemetryRestartDue = [DateTime]::MinValue
$script:LastMetric = $null
$script:CpuHistory = New-Object System.Collections.ArrayList
$script:MemoryHistory = New-Object System.Collections.ArrayList
$script:NetworkHistory = New-Object System.Collections.ArrayList
$script:CommandSessions = New-Object System.Collections.ArrayList
$script:Forward = $null
$script:ForwardDesired = ([bool]$script:Config.forward.autoStart) -and (-not $NoAutoStart)
$script:ForwardExpectedStop = $false
$script:ForwardRecoveryActive = $false
$script:ForwardRestartDue = [DateTime]::MinValue
$script:ForwardAttempt = 0
$script:ForwardStarted = $null
$script:Btop = $null
$script:BtopDesired = ([bool]$script:Config.btop.enabled) -and (-not $NoAutoStart)
$script:BtopExpectedStop = $false
$script:BtopRestartDue = [DateTime]::MinValue
$script:LastEndpointCheck = [DateTime]::MinValue
$script:SshTarget = Resolve-SshTarget -Alias ([string]$script:Config.sshHost)

function Append-TextBox {
    param(
        [System.Windows.Controls.TextBox]$TextBox,
        [string]$Text,
        [int]$MaximumLength = 160000
    )
    if ($null -eq $TextBox -or [string]::IsNullOrEmpty($Text)) { return }
    $TextBox.AppendText($Text)
    if ($TextBox.Text.Length -gt $MaximumLength) {
        $TextBox.Text = $TextBox.Text.Substring($TextBox.Text.Length - [int]($MaximumLength * 0.7))
    }
    $TextBox.ScrollToEnd()
}

function New-SshProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Arguments,
        [switch]$RedirectInput,
        [switch]$Visible
    )
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $script:SshExe
    $info.Arguments = $Arguments
    $info.UseShellExecute = $false
    $info.CreateNoWindow = -not $Visible
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.RedirectStandardInput = [bool]$RedirectInput
    $info.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $info.StandardErrorEncoding = [System.Text.Encoding]::UTF8
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $info
    if (-not $process.Start()) { throw '无法启动 ssh.exe' }
    return $process
}

function Stop-ManagedProcess {
    param($Holder)
    if ($null -eq $Holder) { return }
    try {
        $process = if ($Holder.PSObject.Properties.Name -contains 'Process') { $Holder.Process } else { $Holder }
        if ($null -ne $process -and -not $process.HasExited) {
            $process.Kill()
            [void]$process.WaitForExit(2500)
        }
        if ($null -ne $process) { $process.Dispose() }
    }
    catch {}
}

function Start-Telemetry {
    if ($script:ShuttingDown) { return }
    if ($script:Telemetry -and -not $script:Telemetry.Process.HasExited) { return }
    try {
        $remote = "env LAB_MONITOR_INTERVAL=$([double]$script:Config.telemetryIntervalSeconds) LAB_MONITOR_FORWARD_PORT=$([int]$script:Config.forward.remotePort) python3 -u -"
        $arguments = "-T -o BatchMode=yes -o ConnectTimeout=15 -o ClearAllForwardings=yes $($script:Config.sshHost) `"$remote`""
        $process = New-SshProcess -Arguments $arguments -RedirectInput
        $source = Get-Content -Raw -LiteralPath $script:TelemetryScriptPath
        $process.StandardInput.Write($source)
        $process.StandardInput.Close()
        $script:Telemetry = [pscustomobject]@{
            Process = $process
            OutTask = $process.StandardOutput.ReadLineAsync()
            ErrTask = $process.StandardError.ReadLineAsync()
            ErrorText = New-Object System.Text.StringBuilder
        }
        $script:TelemetryLastStart = Get-Date
        $script:TelemetryRestartDue = [DateTime]::MinValue
        Set-StatusChip $script:TelemetryStatusText '正在连接…' 'warn'
        Write-AppLog "性能遥测 SSH 会话已启动（PID $($process.Id)）。"
    }
    catch {
        Set-StatusChip $script:TelemetryStatusText '启动失败' 'bad'
        Write-AppLog "性能遥测启动失败：$($_.Exception.Message)" 'ERROR'
        $script:TelemetryRestartDue = (Get-Date).AddSeconds(5)
    }
}

function Stop-Telemetry {
    Stop-ManagedProcess $script:Telemetry
    $script:Telemetry = $null
}

function Add-HistoryValue {
    param([System.Collections.ArrayList]$List, [double]$Value)
    [void]$List.Add($Value)
    while ($List.Count -gt 90) { $List.RemoveAt(0) }
}

function Draw-LineChart {
    param(
        [System.Windows.Controls.Canvas]$Canvas,
        [System.Collections.IList]$Values,
        [double]$Maximum,
        [string]$Color
    )
    $Canvas.Children.Clear()
    $width = [Math]::Max(10, $Canvas.ActualWidth)
    $height = [Math]::Max(10, $Canvas.ActualHeight)
    foreach ($ratio in @(0.25, 0.5, 0.75)) {
        $line = New-Object System.Windows.Shapes.Line
        $line.X1 = 0; $line.X2 = $width
        $line.Y1 = $height * $ratio; $line.Y2 = $height * $ratio
        $line.Stroke = [System.Windows.Media.BrushConverter]::new().ConvertFromString('#20344D')
        $line.StrokeThickness = 1
        [void]$Canvas.Children.Add($line)
    }
    if ($Values.Count -lt 2) { return }
    $polyline = New-Object System.Windows.Shapes.Polyline
    $polyline.Stroke = [System.Windows.Media.BrushConverter]::new().ConvertFromString($Color)
    $polyline.StrokeThickness = 2
    $polyline.StrokeLineJoin = 'Round'
    for ($index = 0; $index -lt $Values.Count; $index++) {
        $x = $index * $width / [Math]::Max(1, $Values.Count - 1)
        $normalized = [Math]::Min(1, [Math]::Max(0, [double]$Values[$index] / [Math]::Max(0.001, $Maximum)))
        $y = $height - ($normalized * ($height - 4)) - 2
        $polyline.Points.Add((New-Object System.Windows.Point($x, $y)))
    }
    [void]$Canvas.Children.Add($polyline)
}

function New-LabelText {
    param([string]$Text, [string]$Color = '#C8D7E5', [double]$Size = 13)
    $block = New-Object System.Windows.Controls.TextBlock
    $block.Text = $Text
    $block.Foreground = [System.Windows.Media.BrushConverter]::new().ConvertFromString($Color)
    $block.FontSize = $Size
    return $block
}

function Update-Dashboard {
    param($Data)
    $script:LastMetric = $Data
    $script:TelemetryLastData = Get-Date

    $cpu = [double]$Data.cpu.percent
    $memory = [double]$Data.memory.percent
    $script:CpuValueText.Text = "{0:N1} %" -f $cpu
    $script:CpuProgress.Value = $cpu
    $temperature = if ($null -ne $Data.cpu.temperature_c) { " · {0:N0} °C" -f [double]$Data.cpu.temperature_c } else { '' }
    $script:CpuDetailText.Text = "{0} 核{1}" -f $Data.cpu.cores, $temperature

    $script:MemoryValueText.Text = "{0:N1} %" -f $memory
    $script:MemoryProgress.Value = $memory
    $script:MemoryDetailText.Text = "{0} / {1}" -f (Format-Bytes $Data.memory.used_bytes), (Format-Bytes $Data.memory.total_bytes)
    $script:LoadValueText.Text = "{0:N2}" -f [double]$Data.cpu.load1
    $script:LoadDetailText.Text = "{0:N2} / {1:N2} / {2:N2} · {3} 核" -f [double]$Data.cpu.load1, [double]$Data.cpu.load5, [double]$Data.cpu.load15, $Data.cpu.cores
    $script:UptimeValueText.Text = Format-Uptime ([int64]$Data.uptime_seconds)
    $script:HostValueText.Text = [string]$Data.hostname

    Add-HistoryValue $script:CpuHistory $cpu
    Add-HistoryValue $script:MemoryHistory $memory
    $networkTotal = [double]$Data.network.receive_bytes_per_second + [double]$Data.network.send_bytes_per_second
    Add-HistoryValue $script:NetworkHistory $networkTotal
    $networkMax = [Math]::Max(1MB, ($script:NetworkHistory | Measure-Object -Maximum).Maximum)
    Draw-LineChart $script:CpuChart $script:CpuHistory 100 '#38BDF8'
    Draw-LineChart $script:MemoryChart $script:MemoryHistory 100 '#A78BFA'
    Draw-LineChart $script:NetworkChart $script:NetworkHistory $networkMax '#34D399'
    $script:NetworkTitleText.Text = "网络吞吐  ↓ $(Format-Rate $Data.network.receive_bytes_per_second)  ↑ $(Format-Rate $Data.network.send_bytes_per_second)"

    $script:ProcessGrid.ItemsSource = @($Data.processes)

    $script:GpuPanel.Children.Clear()
    if (@($Data.gpus).Count -eq 0) {
        [void]$script:GpuPanel.Children.Add((New-LabelText '未检测到 NVIDIA GPU' '#8298AE'))
    }
    else {
        foreach ($gpu in @($Data.gpus)) {
            $title = New-LabelText ("GPU {0} · {1}" -f $gpu.index, $gpu.name) '#DDEAF6' 13
            $title.FontWeight = 'SemiBold'
            $title.Margin = '0,7,0,2'
            [void]$script:GpuPanel.Children.Add($title)
            [void]$script:GpuPanel.Children.Add((New-LabelText ("利用率 {0:N0}% · 显存 {1:N0}/{2:N0} MiB" -f [double]$gpu.util_percent, [double]$gpu.memory_used_mib, [double]$gpu.memory_total_mib) '#93A9BE' 12))
            [void]$script:GpuPanel.Children.Add((New-LabelText ("温度 {0:N0} °C · 功耗 {1:N0} W" -f [double]$gpu.temperature_c, [double]$gpu.power_w) '#93A9BE' 12))
            $bar = New-Object System.Windows.Controls.ProgressBar
            $bar.Value = [double]$gpu.util_percent
            $bar.Margin = '0,5,0,4'
            [void]$script:GpuPanel.Children.Add($bar)
        }
    }

    $script:DiskPanel.Children.Clear()
    foreach ($disk in @($Data.disks)) {
        [void]$script:DiskPanel.Children.Add((New-LabelText ("{0} · {1:N1}%" -f $disk.mount, [double]$disk.percent) '#DDEAF6' 13))
        [void]$script:DiskPanel.Children.Add((New-LabelText ("{0} / {1}" -f (Format-Bytes $disk.used_bytes), (Format-Bytes $disk.total_bytes)) '#93A9BE' 12))
        $bar = New-Object System.Windows.Controls.ProgressBar
        $bar.Value = [double]$disk.percent
        $bar.Foreground = if ([double]$disk.percent -ge 90) { '#FB7185' } elseif ([double]$disk.percent -ge 80) { '#FBBF24' } else { '#38BDF8' }
        $bar.Margin = '0,5,0,8'
        [void]$script:DiskPanel.Children.Add($bar)
    }

    Set-StatusChip $script:TelemetryStatusText ("实时 · {0:HH:mm:ss}" -f (Get-Date)) 'good'
    $script:FooterStatusText.Text = "最后遥测：{0:yyyy-MM-dd HH:mm:ss} · SSH PID {1}" -f $script:TelemetryLastData, $script:Telemetry.Process.Id
    if ([int]$Data.btop_count -gt 0) {
        Set-StatusChip $script:BtopStatusText ("运行中 · $($Data.btop_count) 个进程") 'good'
    }
    if ([bool]$Data.forward_listening) {
        Set-StatusChip $script:ForwardStatusText ("已监听 127.0.0.1:$($script:Config.forward.remotePort)") 'good'
        $script:RemoteForwardDetailText.Text = "正常 · 127.0.0.1:$($script:Config.forward.remotePort)"
        $script:RemoteForwardDetailText.Foreground = '#4ADE80'
        $script:ForwardAttempt = 0
    }
    elseif ($script:Forward -and -not $script:Forward.Process.HasExited) {
        Set-StatusChip $script:ForwardStatusText 'SSH 活跃，等待监听' 'warn'
        $script:RemoteForwardDetailText.Text = "未监听 · 127.0.0.1:$($script:Config.forward.remotePort)"
        $script:RemoteForwardDetailText.Foreground = '#FBBF24'
    }
}

function Pump-Telemetry {
    if ($null -eq $script:Telemetry) { return }
    $holder = $script:Telemetry
    $count = 0
    while ($holder.OutTask -and $holder.OutTask.IsCompleted -and $count -lt 8) {
        $count++
        try { $line = $holder.OutTask.Result } catch { $line = $null }
        if ($null -eq $line) { $holder.OutTask = $null; break }
        if (-not [string]::IsNullOrWhiteSpace($line)) {
            try { Update-Dashboard ($line | ConvertFrom-Json) }
            catch { Write-AppLog "遥测数据解析失败：$($_.Exception.Message)" 'WARN' }
        }
        $holder.OutTask = $holder.Process.StandardOutput.ReadLineAsync()
    }
    $count = 0
    while ($holder.ErrTask -and $holder.ErrTask.IsCompleted -and $count -lt 8) {
        $count++
        try { $line = $holder.ErrTask.Result } catch { $line = $null }
        if ($null -eq $line) { $holder.ErrTask = $null; break }
        if (-not [string]::IsNullOrWhiteSpace($line)) { [void]$holder.ErrorText.AppendLine($line) }
        $holder.ErrTask = $holder.Process.StandardError.ReadLineAsync()
    }
    if ($holder.Process.HasExited -and $null -eq $holder.OutTask -and $null -eq $holder.ErrTask) {
        $errorText = $holder.ErrorText.ToString().Trim()
        $exitCode = $holder.Process.ExitCode
        Stop-ManagedProcess $holder
        $script:Telemetry = $null
        if (-not $script:ShuttingDown) {
            Set-StatusChip $script:TelemetryStatusText "已断开（$exitCode）" 'bad'
            Write-AppLog "遥测会话退出，代码 $exitCode。$errorText" 'WARN'
            $script:TelemetryRestartDue = (Get-Date).AddSeconds(3)
        }
    }
}

function Start-RemoteCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [string]$Label = '远程命令',
        [string]$Kind = 'command',
        [switch]$Quiet
    )
    try {
        $arguments = "-T -o BatchMode=yes -o ConnectTimeout=15 -o ClearAllForwardings=yes $($script:Config.sshHost) `"bash -s`""
        $process = New-SshProcess -Arguments $arguments -RedirectInput
        $process.StandardInput.NewLine = "`n"
        $process.StandardInput.WriteLine('set -o pipefail')
        $process.StandardInput.WriteLine($Command)
        $process.StandardInput.Close()
        $session = [pscustomobject]@{
            Process = $process
            OutTask = $process.StandardOutput.ReadLineAsync()
            ErrTask = $process.StandardError.ReadLineAsync()
            Label = $Label
            Kind = $Kind
            Quiet = [bool]$Quiet
            Output = New-Object System.Text.StringBuilder
            ErrorText = New-Object System.Text.StringBuilder
        }
        [void]$script:CommandSessions.Add($session)
        if (-not $Quiet) {
            Append-TextBox $script:CommandOutputBox "`r`n> [$Label] $Command`r`n"
        }
        Write-AppLog -Message ("已发送远程命令：{0}" -f $Label) -Level 'INFO'
        return $session
    }
    catch {
        Write-AppLog -Message ("远程命令启动失败（{0}）：{1}" -f $Label, $_.Exception.Message) -Level 'ERROR'
        if (-not $Quiet) { Append-TextBox $script:CommandOutputBox "启动失败：$($_.Exception.Message)`r`n" }
        return $null
    }
}

function Complete-RemoteSession {
    param($Session)
    $exitCode = $Session.Process.ExitCode
    $errorText = $Session.ErrorText.ToString().Trim()
    if (-not $Session.Quiet) {
        if (-not [string]::IsNullOrWhiteSpace($errorText)) {
            Append-TextBox $script:CommandOutputBox ("[stderr] {0}`r`n" -f $errorText)
        }
        Append-TextBox $script:CommandOutputBox ("[退出码 {0}]`r`n" -f $exitCode)
    }
    if ($Session.Kind -eq 'forward-release-restart') {
        Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} 远端端口释放完成，退出码 {1}。{2}`r`n" -f (Get-Date), $exitCode, $errorText)
        $script:ForwardRecoveryActive = $false
        if ($script:ForwardDesired -and $script:ForwardAutoRestartCheck.IsChecked) {
            $delay = [Math]::Min([int]$script:Config.forward.maxRestartDelaySeconds, [Math]::Pow(2, [Math]::Min(6, $script:ForwardAttempt)))
            $script:ForwardRestartDue = (Get-Date).AddSeconds($delay)
            Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} 将在 {1:N0} 秒后重连。`r`n" -f (Get-Date), $delay)
        }
    }
    elseif ($Session.Kind -eq 'forward-release-manual') {
        Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} 手动释放完成，退出码 {1}。{2}`r`n" -f (Get-Date), $exitCode, $errorText)
    }
    $completedMessage = "远程任务 [$($Session.Label)] 完成，退出码 $exitCode。"
    Write-AppLog -Message $completedMessage -Level 'INFO'
    Stop-ManagedProcess $Session
    [void]$script:CommandSessions.Remove($Session)
}

function Pump-CommandSessions {
    foreach ($session in @($script:CommandSessions)) {
        $count = 0
        while ($session.OutTask -and $session.OutTask.IsCompleted -and $count -lt 80) {
            $count++
            try { $line = $session.OutTask.Result } catch { $line = $null }
            if ($null -eq $line) { $session.OutTask = $null; break }
            [void]$session.Output.AppendLine($line)
            if (-not $session.Quiet) { Append-TextBox $script:CommandOutputBox ($line + "`r`n") }
            $session.OutTask = $session.Process.StandardOutput.ReadLineAsync()
        }
        $count = 0
        while ($session.ErrTask -and $session.ErrTask.IsCompleted -and $count -lt 80) {
            $count++
            try { $line = $session.ErrTask.Result } catch { $line = $null }
            if ($null -eq $line) { $session.ErrTask = $null; break }
            [void]$session.ErrorText.AppendLine($line)
            if (-not $session.Quiet) { Append-TextBox $script:CommandOutputBox ("[stderr] $line`r`n") }
            $session.ErrTask = $session.Process.StandardError.ReadLineAsync()
        }
        if ($session.Process.HasExited -and $null -eq $session.OutTask -and $null -eq $session.ErrTask) {
            Complete-RemoteSession $session
        }
    }
}

function Stop-Btop {
    param([switch]$Expected)
    if ($Expected) { $script:BtopExpectedStop = $true }
    Stop-ManagedProcess $script:Btop
    $script:Btop = $null
    Set-StatusChip $script:BtopStatusText '已停止' 'idle'
}

function Start-BtopHidden {
    if ($script:ShuttingDown -or -not $script:BtopDesired) { return }
    if ($script:Btop -and -not $script:Btop.Process.HasExited) { return }
    try {
        $cycle = if ($script:Config.btop.PSObject.Properties.Name -contains 'restartCycleSeconds') { [int]$script:Config.btop.restartCycleSeconds } else { 900 }
        $command = "while true; do TERM=xterm-256color timeout --signal=TERM $cycle $($script:Config.btop.command) >/dev/null 2>&1; sleep 2; done"
        $arguments = "-tt -o BatchMode=yes -o ConnectTimeout=15 -o ClearAllForwardings=yes $($script:Config.sshHost) `"$command`""
        $process = New-SshProcess -Arguments $arguments
        $script:Btop = [pscustomobject]@{
            Process = $process
            Mode = 'hidden'
            ErrTask = $process.StandardError.ReadLineAsync()
            ErrorText = New-Object System.Text.StringBuilder
            Started = Get-Date
        }
        $script:BtopExpectedStop = $false
        $script:BtopRestartDue = [DateTime]::MinValue
        Set-StatusChip $script:BtopStatusText '启动中…' 'warn'
        $script:BtopDetailText.Text = "隐藏保活会话 · PID $($process.Id) · 主动轮换周期 $cycle 秒"
        Append-TextBox $script:BtopLogBox ("{0:HH:mm:ss} 启动隐藏 btop 保活会话（PID {1}）。`r`n" -f (Get-Date), $process.Id)
        Write-AppLog "btop 隐藏保活会话已启动（PID $($process.Id)）。"
    }
    catch {
        Set-StatusChip $script:BtopStatusText '启动失败' 'bad'
        Append-TextBox $script:BtopLogBox ("{0:HH:mm:ss} 启动失败：{1}`r`n" -f (Get-Date), $_.Exception.Message)
        Write-AppLog "btop 启动失败：$($_.Exception.Message)" 'ERROR'
        $script:BtopRestartDue = (Get-Date).AddSeconds(5)
    }
}

function Start-BtopInteractive {
    $script:BtopDesired = $true
    Stop-Btop -Expected
    try {
        $cycle = if ($script:Config.btop.PSObject.Properties.Name -contains 'restartCycleSeconds') { [int]$script:Config.btop.restartCycleSeconds } else { 900 }
        $remote = "while true; do TERM=xterm-256color timeout --signal=TERM $cycle $($script:Config.btop.command); printf '\n[btop watchdog] restarting in 2 seconds...\n'; sleep 2; done"
        $remoteBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($remote))
        $localScript = @"
`$Host.UI.RawUI.WindowTitle = 'lab · btop（关闭窗口以返回隐藏保活）'
`$remoteCommand = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$remoteBase64'))
& '$script:SshExe' -tt -o BatchMode=yes -o ClearAllForwardings=yes '$($script:Config.sshHost)' `$remoteCommand
Write-Host ''
Write-Host 'btop SSH 会话已结束，窗口即将关闭。' -ForegroundColor Yellow
Start-Sleep -Seconds 3
"@
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($localScript))
        $process = Start-Process -FilePath 'powershell.exe' -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $encoded -WindowStyle Normal -PassThru
        $script:Btop = [pscustomobject]@{
            Process = $process
            Mode = 'interactive'
            ErrTask = $null
            ErrorText = New-Object System.Text.StringBuilder
            Started = Get-Date
        }
        $script:BtopExpectedStop = $false
        Set-StatusChip $script:BtopStatusText '交互终端' 'good'
        $script:BtopDetailText.Text = "交互终端 · 本地 PID $($process.Id) · 关闭终端后恢复隐藏保活"
        Append-TextBox $script:BtopLogBox ("{0:HH:mm:ss} 已打开交互 btop 终端。`r`n" -f (Get-Date))
    }
    catch {
        Write-AppLog "打开 btop 交互终端失败：$($_.Exception.Message)" 'ERROR'
        $script:BtopRestartDue = (Get-Date).AddSeconds(2)
    }
}

function Pump-Btop {
    if ($null -eq $script:Btop) { return }
    if ($script:Btop.ErrTask -and $script:Btop.ErrTask.IsCompleted) {
        try { $line = $script:Btop.ErrTask.Result } catch { $line = $null }
        if ($null -eq $line) { $script:Btop.ErrTask = $null }
        else {
            if (-not [string]::IsNullOrWhiteSpace($line)) { [void]$script:Btop.ErrorText.AppendLine($line) }
            $script:Btop.ErrTask = $script:Btop.Process.StandardError.ReadLineAsync()
        }
    }
    if ($script:Btop.Process.HasExited) {
        $mode = $script:Btop.Mode
        $expected = $script:BtopExpectedStop
        $errorText = $script:Btop.ErrorText.ToString().Trim()
        Stop-ManagedProcess $script:Btop
        $script:Btop = $null
        if (-not $script:ShuttingDown) {
            Append-TextBox $script:BtopLogBox ("{0:HH:mm:ss} {1} btop 会话退出。{2}`r`n" -f (Get-Date), $mode, $errorText)
            if ($script:BtopDesired -and $script:BtopAutoRestartCheck.IsChecked) {
                $script:BtopRestartDue = (Get-Date).AddSeconds(2)
                Set-StatusChip $script:BtopStatusText '等待自动重启' 'warn'
            }
            elseif (-not $expected) {
                Set-StatusChip $script:BtopStatusText '意外停止' 'bad'
            }
        }
    }
}

function Start-Forward {
    if ($script:ShuttingDown -or -not $script:ForwardDesired) { return }
    if ($script:Forward -and -not $script:Forward.Process.HasExited) { return }
    try {
        $arguments = "-NT -o BatchMode=yes -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=3 $($script:Config.forwardHost)"
        $process = New-SshProcess -Arguments $arguments
        $script:Forward = [pscustomobject]@{
            Process = $process
            OutTask = $process.StandardOutput.ReadLineAsync()
            ErrTask = $process.StandardError.ReadLineAsync()
            ErrorText = New-Object System.Text.StringBuilder
        }
        $script:ForwardExpectedStop = $false
        $script:ForwardStarted = Get-Date
        $script:ForwardAttempt++
        $script:ForwardRestartDue = [DateTime]::MinValue
        Set-StatusChip $script:ForwardStatusText '正在建立…' 'warn'
        Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} 启动 {1}（PID {2}，第 {3} 次尝试）。`r`n" -f (Get-Date), $script:Config.forwardHost, $process.Id, $script:ForwardAttempt)
        Write-AppLog "端口转发已启动（PID $($process.Id)）。"
    }
    catch {
        Set-StatusChip $script:ForwardStatusText '启动失败' 'bad'
        Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} 启动失败：{1}`r`n" -f (Get-Date), $_.Exception.Message)
        Write-AppLog "端口转发启动失败：$($_.Exception.Message)" 'ERROR'
        $script:ForwardRestartDue = (Get-Date).AddSeconds(5)
    }
}

function Stop-Forward {
    param([switch]$Expected)
    if ($Expected) { $script:ForwardExpectedStop = $true }
    Stop-ManagedProcess $script:Forward
    $script:Forward = $null
    if ($Expected) {
        Set-StatusChip $script:ForwardStatusText '已停止' 'idle'
        Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} 已停止本程序创建的转发进程。`r`n" -f (Get-Date))
    }
}

function Request-ForwardRecovery {
    param([string]$Reason = '连接异常')
    if ($script:ForwardRecoveryActive -or $script:ShuttingDown) { return }
    $script:ForwardRecoveryActive = $true
    Stop-Forward
    Set-StatusChip $script:ForwardStatusText '正在释放远端端口' 'warn'
    Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} {1}；先尝试释放远端端口，再重连。`r`n" -f (Get-Date), $Reason)
    $session = Start-RemoteCommand -Command ([string]$script:Config.forward.releaseCommand) -Label '释放反向转发端口' -Kind 'forward-release-restart' -Quiet
    if ($null -eq $session) {
        $script:ForwardRecoveryActive = $false
        $script:ForwardRestartDue = (Get-Date).AddSeconds(5)
    }
}

function Pump-Forward {
    if ($null -eq $script:Forward) { return }
    $holder = $script:Forward
    if ($holder.OutTask -and $holder.OutTask.IsCompleted) {
        try { $line = $holder.OutTask.Result } catch { $line = $null }
        if ($null -eq $line) { $holder.OutTask = $null }
        else { $holder.OutTask = $holder.Process.StandardOutput.ReadLineAsync() }
    }
    if ($holder.ErrTask -and $holder.ErrTask.IsCompleted) {
        try { $line = $holder.ErrTask.Result } catch { $line = $null }
        if ($null -eq $line) { $holder.ErrTask = $null }
        else {
            if (-not [string]::IsNullOrWhiteSpace($line)) {
                [void]$holder.ErrorText.AppendLine($line)
                Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} {1}`r`n" -f (Get-Date), $line)
            }
            $holder.ErrTask = $holder.Process.StandardError.ReadLineAsync()
        }
    }
    if ($holder.Process.HasExited -and $null -eq $holder.OutTask -and $null -eq $holder.ErrTask) {
        $exitCode = $holder.Process.ExitCode
        $errorText = $holder.ErrorText.ToString().Trim()
        $expected = $script:ForwardExpectedStop
        Stop-ManagedProcess $holder
        $script:Forward = $null
        if (-not $script:ShuttingDown -and -not $expected) {
            Set-StatusChip $script:ForwardStatusText "意外断开（$exitCode）" 'bad'
            Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} 转发意外退出，代码 {1}。{2}`r`n" -f (Get-Date), $exitCode, $errorText)
            Write-AppLog "端口转发意外退出，代码 $exitCode。$errorText" 'WARN'
            if ($script:ForwardDesired -and $script:ForwardAutoRestartCheck.IsChecked) {
                Request-ForwardRecovery '转发进程意外退出'
            }
        }
    }
}

function Test-Endpoints {
    $sshOk = Test-TcpEndpoint -Address $script:SshTarget.HostName -Port $script:SshTarget.Port -TimeoutMilliseconds 1200
    if ($sshOk) {
        Set-StatusChip $script:SshStatusText ("可达 · {0}:{1}" -f $script:SshTarget.HostName, $script:SshTarget.Port) 'good'
        $script:EndpointDetailText.Text = "可达 · $($script:SshTarget.HostName):$($script:SshTarget.Port)"
        $script:EndpointDetailText.Foreground = '#4ADE80'
    }
    else {
        Set-StatusChip $script:SshStatusText ("不可达 · {0}:{1}" -f $script:SshTarget.HostName, $script:SshTarget.Port) 'bad'
        $script:EndpointDetailText.Text = "不可达 · $($script:SshTarget.HostName):$($script:SshTarget.Port)"
        $script:EndpointDetailText.Foreground = '#FB7185'
    }
    $localOk = Test-TcpEndpoint -Address ([string]$script:Config.forward.localTargetAddress) -Port ([int]$script:Config.forward.localTargetPort) -TimeoutMilliseconds 500
    if ($localOk) {
        $script:LocalTargetDetailText.Text = "可达 · $($script:Config.forward.localTargetAddress):$($script:Config.forward.localTargetPort)"
        $script:LocalTargetDetailText.Foreground = '#4ADE80'
    }
    else {
        $script:LocalTargetDetailText.Text = "不可达 · $($script:Config.forward.localTargetAddress):$($script:Config.forward.localTargetPort)"
        $script:LocalTargetDetailText.Foreground = '#FBBF24'
    }
    $script:LastEndpointCheck = Get-Date
}

function Test-DangerousCommand {
    param([string]$Command)
    foreach ($pattern in @($script:Config.dangerousCommandPatterns)) {
        if ($Command -match [string]$pattern) { return $true }
    }
    return $false
}

function Confirm-DangerousAction {
    param(
        [string]$Title,
        [string]$Command,
        [string]$Token = 'EXECUTE lab'
    )
    $message = "这是危险操作，可能中断任务、连接或造成数据损失。`n`n目标：$($script:Config.sshHost)`n命令：$Command`n`n是否继续到第二次验证？"
    $answer = [System.Windows.MessageBox]::Show($script:Window, $message, $Title, 'YesNo', 'Warning')
    if ($answer -ne 'Yes') { return $false }
    $typed = [Microsoft.VisualBasic.Interaction]::InputBox(
        "请准确输入以下验证文本：`r`n`r`n$Token",
        "重复验证 · $Title",
        ''
    )
    if ($typed -cne $Token) {
        [void][System.Windows.MessageBox]::Show($script:Window, '验证文本不匹配，操作已取消。', '已取消', 'OK', 'Information')
        return $false
    }
    return $true
}

function Start-DangerousInteractiveCommand {
    param([string]$Command, [string]$Label)
    $bytes = [Text.Encoding]::UTF8.GetBytes($Command)
    $commandBase64 = [Convert]::ToBase64String($bytes)
    $labelBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Label))
    $remote = "printf %s $commandBase64 | base64 -d | bash"
    $remoteBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($remote))
    $localScript = @"
`$displayCommand = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$commandBase64'))
`$displayLabel = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$labelBase64'))
`$remoteCommand = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$remoteBase64'))
`$Host.UI.RawUI.WindowTitle = '危险命令 · ' + `$displayLabel
Write-Host '目标：$($script:Config.sshHost)' -ForegroundColor Yellow
Write-Host ('命令：' + `$displayCommand) -ForegroundColor Yellow
Write-Host ''
& '$script:SshExe' -tt -o ClearAllForwardings=yes '$($script:Config.sshHost)' `$remoteCommand
`$result = `$LASTEXITCODE
Write-Host ''
Write-Host "SSH 退出码：`$result。按回车关闭窗口。" -ForegroundColor Cyan
Read-Host
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($localScript))
    [void](Start-Process -FilePath 'powershell.exe' -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $encoded -WindowStyle Normal)
    Write-AppLog "危险命令通过交互终端启动：$Label" 'WARN'
}

function Invoke-UserCommand {
    param(
        [string]$Command,
        [string]$Label,
        [string]$Risk = 'safe',
        [string]$ConfirmToken = ''
    )
    if ([string]::IsNullOrWhiteSpace($Command)) { return }
    $dangerous = ($Risk -eq 'danger') -or (Test-DangerousCommand $Command)
    if ($dangerous) {
        $token = if ([string]::IsNullOrWhiteSpace($ConfirmToken)) { 'EXECUTE lab' } else { $ConfirmToken }
        if (Confirm-DangerousAction -Title $Label -Command $Command -Token $token) {
            Start-DangerousInteractiveCommand -Command $Command -Label $Label
        }
        return
    }
    [void](Start-RemoteCommand -Command $Command -Label $Label)
}

$script:PresetList.add_SelectionChanged({
    $preset = $script:PresetList.SelectedItem
    if ($null -eq $preset) { return }
    $script:PresetNameText.Text = [string]$preset.name
    $script:PresetDescriptionText.Text = [string]$preset.description
    if ([string]$preset.risk -eq 'danger') {
        $script:PresetRiskText.Text = '危险命令 · 需要警告确认和验证文本'
        $script:PresetRiskText.Foreground = '#FB7185'
    }
    else {
        $script:PresetRiskText.Text = '安全只读命令 · 点击即执行'
        $script:PresetRiskText.Foreground = '#4ADE80'
    }
})

$script:RunPresetButton.add_Click({
    $preset = $script:PresetList.SelectedItem
    if ($null -eq $preset) {
        [void][System.Windows.MessageBox]::Show($script:Window, '请先选择一个命令预设。', '未选择', 'OK', 'Information')
        return
    }
    $token = if ($preset.PSObject.Properties.Name -contains 'confirmToken') { [string]$preset.confirmToken } else { '' }
    Invoke-UserCommand -Command ([string]$preset.command) -Label ([string]$preset.name) -Risk ([string]$preset.risk) -ConfirmToken $token
})

$script:RunCustomButton.add_Click({
    Invoke-UserCommand -Command $script:CustomCommandBox.Text -Label '自定义命令'
})

$script:ClearCommandOutputButton.add_Click({ $script:CommandOutputBox.Clear() })
$script:ClearLogViewButton.add_Click({ $script:LogBox.Clear() })

$script:BtopStartButton.add_Click({
    $script:BtopDesired = $true
    Start-BtopHidden
})
$script:BtopRestartButton.add_Click({
    $script:BtopDesired = $true
    Stop-Btop -Expected
    $script:BtopRestartDue = (Get-Date).AddSeconds(1)
    Append-TextBox $script:BtopLogBox ("{0:HH:mm:ss} 用户请求重启 btop。`r`n" -f (Get-Date))
})
$script:BtopInteractiveButton.add_Click({ Start-BtopInteractive })
$script:BtopStopButton.add_Click({
    $script:BtopDesired = $false
    Stop-Btop -Expected
    Append-TextBox $script:BtopLogBox ("{0:HH:mm:ss} 用户停止 btop 保活。`r`n" -f (Get-Date))
})

$script:ForwardStartButton.add_Click({
    $script:ForwardDesired = $true
    Start-Forward
})
$script:ForwardStopButton.add_Click({
    $script:ForwardDesired = $false
    $script:ForwardRecoveryActive = $false
    Stop-Forward -Expected
})
$script:ForwardTestButton.add_Click({
    Test-Endpoints
    if ($script:LastMetric) { Update-Dashboard $script:LastMetric }
    Append-TextBox $script:ForwardLogBox ("{0:HH:mm:ss} 用户执行了连接测试。`r`n" -f (Get-Date))
})
$script:ForwardRestartButton.add_Click({
    if (Confirm-DangerousAction -Title '释放并重启端口转发' -Command ([string]$script:Config.forward.releaseCommand) -Token 'RESTART FORWARD') {
        $script:ForwardDesired = $true
        Stop-Forward -Expected
        $script:ForwardExpectedStop = $false
        Request-ForwardRecovery '用户请求释放并重启'
    }
})
$script:ForwardReleaseButton.add_Click({
    if (Confirm-DangerousAction -Title '仅释放远端端口' -Command ([string]$script:Config.forward.releaseCommand) -Token 'RELEASE 17890') {
        $script:ForwardDesired = $false
        Stop-Forward -Expected
        [void](Start-RemoteCommand -Command ([string]$script:Config.forward.releaseCommand) -Label '手动释放远端端口' -Kind 'forward-release-manual' -Quiet)
    }
})

$script:RefreshAllButton.add_Click({
    Test-Endpoints
    if ($null -eq $script:Telemetry -or $script:Telemetry.Process.HasExited) { Start-Telemetry }
    if ($script:BtopDesired -and ($null -eq $script:Btop -or $script:Btop.Process.HasExited)) { Start-BtopHidden }
    if ($script:ForwardDesired -and ($null -eq $script:Forward -or $script:Forward.Process.HasExited)) { Start-Forward }
})

$script:UiTimer = New-Object System.Windows.Threading.DispatcherTimer
$script:UiTimer.Interval = [TimeSpan]::FromMilliseconds(350)
$script:UiTimer.add_Tick({
    try {
        Pump-Telemetry
        Pump-CommandSessions
        Pump-Btop
        Pump-Forward

        $now = Get-Date
        if (($now - $script:LastEndpointCheck).TotalSeconds -ge [double]$script:Config.endpointCheckSeconds) {
            Test-Endpoints
        }

        if ($null -eq $script:Telemetry -and $now -ge $script:TelemetryRestartDue) {
            Start-Telemetry
        }
        elseif ($script:Telemetry -and $script:TelemetryLastData -and
            ($now - $script:TelemetryLastData).TotalSeconds -gt [double]$script:Config.telemetryStaleSeconds) {
            Write-AppLog '遥测超过阈值未更新，正在重启会话。' 'WARN'
            Stop-Telemetry
            $script:TelemetryRestartDue = $now.AddSeconds(2)
        }

        if ($script:BtopDesired -and $script:BtopAutoRestartCheck.IsChecked -and
            $null -eq $script:Btop -and $now -ge $script:BtopRestartDue) {
            Start-BtopHidden
        }
        if ($script:Btop -and $script:LastMetric -and [int]$script:LastMetric.btop_count -eq 0 -and
            ($now - $script:Btop.Started).TotalSeconds -gt 12) {
            Append-TextBox $script:BtopLogBox ("{0:HH:mm:ss} 远端未发现 btop 进程，重启保活会话。`r`n" -f $now)
            Stop-Btop -Expected
            $script:BtopRestartDue = $now.AddSeconds(2)
        }

        if ($script:ForwardDesired -and -not $script:ForwardRecoveryActive -and
            $null -eq $script:Forward -and $now -ge $script:ForwardRestartDue) {
            Start-Forward
        }
    }
    catch {
        Write-AppLog -Message ("UI 看门狗异常：{0} | {1}" -f $_.Exception.Message, $_.ScriptStackTrace) -Level 'ERROR'
    }
})

$script:Window.add_ContentRendered({
    Write-AppLog "程序启动。SSH 别名：$($script:Config.sshHost)；转发别名：$($script:Config.forwardHost)。"
    Test-Endpoints
    Start-Telemetry
    if ($script:BtopDesired) { Start-BtopHidden }
    if ($script:ForwardDesired) { Start-Forward }
    $script:UiTimer.Start()
})

$script:Window.add_Closing({
    $script:ShuttingDown = $true
    $script:UiTimer.Stop()
    Write-AppLog '程序正在关闭，停止本程序创建的后台会话。'
    $script:ForwardExpectedStop = $true
    $script:BtopExpectedStop = $true
    Stop-Forward -Expected
    Stop-Btop -Expected
    Stop-Telemetry
    foreach ($session in @($script:CommandSessions)) {
        Stop-ManagedProcess $session
    }
    $script:CommandSessions.Clear()
})

[void]$script:Window.ShowDialog()
