$name = Read-Host 'Enter your name'
Write-Output "Hello, $name"
Write-Output 'Press any key to finish'
$null = [Console]::ReadKey($true)
