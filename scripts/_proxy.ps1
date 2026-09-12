# _proxy.ps1 —— 自动探测本机 git 代理
#
# 国内开发机通常有 Clash（127.0.0.1:7897）；GitHub CI 上没有 → 自动直连。
# 用法：. "$PSScriptRoot\_proxy.ps1"  然后 git @gitProxyArgs <子命令>

function Get-GitProxyArgs {
    $port = 7897
    $host_ = "127.0.0.1"
    $reachable = $false
    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $async = $client.BeginConnect($host_, $port, $null, $null)
        $reachable = $async.AsyncWaitHandle.WaitOne(300)
        if ($reachable) { try { $client.EndConnect($async) } catch { $reachable = $false } }
        $client.Close()
    } catch { $reachable = $false }

    if ($reachable) {
        Write-Host "  [proxy] 检测到 $host_`:$port，git 走代理" -ForegroundColor DarkGray
        return @("-c", "http.proxy=http://$host_`:$port", "-c", "https.proxy=http://$host_`:$port")
    }
    Write-Host "  [proxy] 无本地代理，git 直连" -ForegroundColor DarkGray
    return @()
}