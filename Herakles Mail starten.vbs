Option Explicit
Dim shell, files, project
Set shell = CreateObject("WScript.Shell")
Set files = CreateObject("Scripting.FileSystemObject")
project = files.GetParentFolderName(WScript.ScriptFullName)
shell.CurrentDirectory = project
shell.Run Chr(34) & project & "\Herakles Mail.exe" & Chr(34), 1, False
