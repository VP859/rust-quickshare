
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName 'ferry' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        'ferry'
        for ($i = 1; $i -lt $commandElements.Count; $i++) {
            $element = $commandElements[$i]
            if ($element -isnot [StringConstantExpressionAst] -or
                $element.StringConstantType -ne [StringConstantType]::BareWord -or
                $element.Value.StartsWith('-') -or
                $element.Value -eq $wordToComplete) {
                break
        }
        $element.Value
    }) -join ';'

    $completions = @(switch ($command) {
        'ferry' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('send', 'send', [CompletionResultType]::ParameterValue, 'Send files and folders to a trusted or nearby device (`--to`), or share one file by a one-time TICKET')
            [CompletionResult]::new('localsend', 'localsend', [CompletionResultType]::ParameterValue, 'Send files to LocalSend apps (iPhone, iPad, Mac, …) and find them on the local network')
            [CompletionResult]::new('integrate', 'integrate', [CompletionResultType]::ParameterValue, 'Add "Send with Ferry" to the file managers of this user (Dolphin, Nautilus, Nemo; Linux)')
            [CompletionResult]::new('history', 'history', [CompletionResultType]::ParameterValue, 'Show finished transfers, newest first, or search them')
            [CompletionResult]::new('web', 'web', [CompletionResultType]::ParameterValue, 'Share files with a web browser on the local network and receive files from it, through a one-time link and QR code (the other device needs no app)')
            [CompletionResult]::new('text', 'text', [CompletionResultType]::ParameterValue, 'Send text or a link; the receiver copies it to the clipboard after accepting')
            [CompletionResult]::new('receive', 'receive', [CompletionResultType]::ParameterValue, 'Download a file shared by TICKET')
            [CompletionResult]::new('daemon', 'daemon', [CompletionResultType]::ParameterValue, 'Run in the background; trusted devices with Auto-Accept are accepted automatically')
            [CompletionResult]::new('tui', 'tui', [CompletionResultType]::ParameterValue, 'Interactive terminal user interface')
            [CompletionResult]::new('ticket', 'ticket', [CompletionResultType]::ParameterValue, 'Show this device''s TICKET (text and QR code) for pairing')
            [CompletionResult]::new('discover', 'discover', [CompletionResultType]::ParameterValue, 'Search for Ferry devices on the local network (own UDP broadcast, no mDNS)')
            [CompletionResult]::new('pair', 'pair', [CompletionResultType]::ParameterValue, 'Pair with a nearby device (name or Node ID from `ferry discover`); the other side confirms and both show the same code')
            [CompletionResult]::new('pairing', 'pairing', [CompletionResultType]::ParameterValue, 'Let unknown devices send pairing requests for a few minutes, although `paired-only` is on (also for a running daemon or TUI); `--close` ends it')
            [CompletionResult]::new('guide', 'guide', [CompletionResultType]::ParameterValue, 'Short answers to the usual questions: sending, pairing, devices without Ferry, iPhone and Mac, local or internet, devices that do not show up, privacy')
            [CompletionResult]::new('name', 'name', [CompletionResultType]::ParameterValue, 'Show or change the name of this device visible to others')
            [CompletionResult]::new('language', 'language', [CompletionResultType]::ParameterValue, 'Show or change the interface language (`en`, `pl`)')
            [CompletionResult]::new('network', 'network', [CompletionResultType]::ParameterValue, 'Show or change the network mode: `local` (no external servers, default), `internet` (also through the free public iroh relays, for devices in different networks), `relay` (also through your own relays) or `relay-only` (only through your own relays, never directly)')
            [CompletionResult]::new('peer', 'peer', [CompletionResultType]::ParameterValue, 'Manage trusted devices')
            [CompletionResult]::new('config', 'config', [CompletionResultType]::ParameterValue, 'Show or change settings (`ferry config list`, `get`, `set`, `unset`, `path`)')
            [CompletionResult]::new('identity', 'identity', [CompletionResultType]::ParameterValue, 'Show, back up or restore this device''s identity (its Node ID key)')
            [CompletionResult]::new('doctor', 'doctor', [CompletionResultType]::ParameterValue, 'Network and configuration diagnostics')
            [CompletionResult]::new('completions', 'completions', [CompletionResultType]::ParameterValue, 'Print the shell completion script, e.g. `ferry completions bash > ~/.local/share/bash-completion/completions/ferry` (packages install it already)')
            [CompletionResult]::new('generate-docs', 'generate-docs', [CompletionResultType]::ParameterValue, 'Write the completion scripts and manual pages to `DIR/completions` and `DIR/man` (for packaging)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;send' {
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 'Alias, nearby device name, Node ID or TICKET of the receiver (it must run `ferry daemon`, the TUI or the GUI). Repeat it to send to several devices at once')
            [CompletionResult]::new('--to', '--to', [CompletionResultType]::ParameterName, 'Alias, nearby device name, Node ID or TICKET of the receiver (it must run `ferry daemon`, the TUI or the GUI). Repeat it to send to several devices at once')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;localsend' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('scan', 'scan', [CompletionResultType]::ParameterValue, 'List the LocalSend apps on the local network. Announces this device''s name to them once, so they answer')
            [CompletionResult]::new('send', 'send', [CompletionResultType]::ParameterValue, 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name')
            [CompletionResult]::new('text', 'text', [CompletionResultType]::ParameterValue, 'Send a text or a link to a LocalSend app, which shows it as a message')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;localsend;scan' {
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 'Seconds to wait for answers')
            [CompletionResult]::new('--timeout', '--timeout', [CompletionResultType]::ParameterName, 'Seconds to wait for answers')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;localsend;send' {
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 'Name or IP address (`192.168.1.20` or `192.168.1.20:53317`) of the LocalSend app, as listed by `ferry localsend scan`')
            [CompletionResult]::new('--to', '--to', [CompletionResultType]::ParameterName, 'Name or IP address (`192.168.1.20` or `192.168.1.20:53317`) of the LocalSend app, as listed by `ferry localsend scan`')
            [CompletionResult]::new('--pin', '--pin', [CompletionResultType]::ParameterName, 'PIN, for apps that require one')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;localsend;text' {
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 'Name or IP address of the LocalSend app, as listed by `ferry localsend scan`')
            [CompletionResult]::new('--to', '--to', [CompletionResultType]::ParameterName, 'Name or IP address of the LocalSend app, as listed by `ferry localsend scan`')
            [CompletionResult]::new('--pin', '--pin', [CompletionResultType]::ParameterName, 'PIN, for apps that require one')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;localsend;help' {
            [CompletionResult]::new('scan', 'scan', [CompletionResultType]::ParameterValue, 'List the LocalSend apps on the local network. Announces this device''s name to them once, so they answer')
            [CompletionResult]::new('send', 'send', [CompletionResultType]::ParameterValue, 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name')
            [CompletionResult]::new('text', 'text', [CompletionResultType]::ParameterValue, 'Send a text or a link to a LocalSend app, which shows it as a message')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;localsend;help;scan' {
            break
        }
        'ferry;localsend;help;send' {
            break
        }
        'ferry;localsend;help;text' {
            break
        }
        'ferry;localsend;help;help' {
            break
        }
        'ferry;integrate' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('--remove', '--remove', [CompletionResultType]::ParameterName, 'Remove the entries again')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;history' {
            [CompletionResult]::new('-n', '-n', [CompletionResultType]::ParameterName, 'How many transfers to show')
            [CompletionResult]::new('--limit', '--limit', [CompletionResultType]::ParameterName, 'How many transfers to show')
            [CompletionResult]::new('--kind', '--kind', [CompletionResultType]::ParameterName, 'Show only texts and links (never their content), or only files')
            [CompletionResult]::new('--send-again', '--send-again', [CompletionResultType]::ParameterName, 'Send the file of transfer N of the list (as numbered) again to the same Ferry device')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('--clear', '--clear', [CompletionResultType]::ParameterName, 'Delete the history')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'ferry;web' {
            [CompletionResult]::new('--minutes', '--minutes', [CompletionResultType]::ParameterName, 'Minutes until the link stops working')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('--no-receive', '--no-receive', [CompletionResultType]::ParameterName, 'Do not let the browser send files here')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;text' {
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 'Alias, nearby device name, Node ID or TICKET of the receiver')
            [CompletionResult]::new('--to', '--to', [CompletionResultType]::ParameterName, 'Alias, nearby device name, Node ID or TICKET of the receiver')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;receive' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;daemon' {
            [CompletionResult]::new('-d', '-d', [CompletionResultType]::ParameterName, 'Folder for received files (default: the saved setting or the download folder)')
            [CompletionResult]::new('--downloads-dir', '--downloads-dir', [CompletionResultType]::ParameterName, 'Folder for received files (default: the saved setting or the download folder)')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;tui' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;ticket' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;discover' {
            [CompletionResult]::new('-t', '-t', [CompletionResultType]::ParameterName, 'Search duration in seconds')
            [CompletionResult]::new('--timeout', '--timeout', [CompletionResultType]::ParameterName, 'Search duration in seconds')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;pair' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('--auto-accept', '--auto-accept', [CompletionResultType]::ParameterName, 'Enable Auto-Accept for the paired device right away')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;pairing' {
            [CompletionResult]::new('-m', '-m', [CompletionResultType]::ParameterName, 'How long pairing stays open, in minutes (at most 30)')
            [CompletionResult]::new('--minutes', '--minutes', [CompletionResultType]::ParameterName, 'How long pairing stays open, in minutes (at most 30)')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('--close', '--close', [CompletionResultType]::ParameterName, 'Close pairing now')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;guide' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;name' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;language' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;network' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;peer' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('add', 'add', [CompletionResultType]::ParameterValue, 'Add a device by TICKET or Node ID and send it a pairing request')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'List trusted devices')
            [CompletionResult]::new('edit', 'edit', [CompletionResultType]::ParameterValue, 'Change the alias or Auto-Accept setting of a device')
            [CompletionResult]::new('verify', 'verify', [CompletionResultType]::ParameterValue, 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match')
            [CompletionResult]::new('remove', 'remove', [CompletionResultType]::ParameterValue, 'Remove a trusted device')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;peer;add' {
            [CompletionResult]::new('-a', '-a', [CompletionResultType]::ParameterName, 'Name for the device')
            [CompletionResult]::new('--alias', '--alias', [CompletionResultType]::ParameterName, 'Name for the device')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('--auto-accept', '--auto-accept', [CompletionResultType]::ParameterName, 'Accept files from this device without asking')
            [CompletionResult]::new('--no-pair', '--no-pair', [CompletionResultType]::ParameterName, 'Do not send a pairing request (trust only on this side)')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;peer;list' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;peer;edit' {
            [CompletionResult]::new('-a', '-a', [CompletionResultType]::ParameterName, 'New alias')
            [CompletionResult]::new('--alias', '--alias', [CompletionResultType]::ParameterName, 'New alias')
            [CompletionResult]::new('--auto-accept', '--auto-accept', [CompletionResultType]::ParameterName, 'New Auto-Accept setting')
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;peer;verify' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('--confirm', '--confirm', [CompletionResultType]::ParameterName, 'The numbers are equal: mark the device as verified')
            [CompletionResult]::new('--unconfirm', '--unconfirm', [CompletionResultType]::ParameterName, 'Remove the verified mark')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;peer;remove' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;peer;help' {
            [CompletionResult]::new('add', 'add', [CompletionResultType]::ParameterValue, 'Add a device by TICKET or Node ID and send it a pairing request')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'List trusted devices')
            [CompletionResult]::new('edit', 'edit', [CompletionResultType]::ParameterValue, 'Change the alias or Auto-Accept setting of a device')
            [CompletionResult]::new('verify', 'verify', [CompletionResultType]::ParameterValue, 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match')
            [CompletionResult]::new('remove', 'remove', [CompletionResultType]::ParameterValue, 'Remove a trusted device')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;peer;help;add' {
            break
        }
        'ferry;peer;help;list' {
            break
        }
        'ferry;peer;help;edit' {
            break
        }
        'ferry;peer;help;verify' {
            break
        }
        'ferry;peer;help;remove' {
            break
        }
        'ferry;peer;help;help' {
            break
        }
        'ferry;config' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'Show every setting')
            [CompletionResult]::new('get', 'get', [CompletionResultType]::ParameterValue, 'Show one setting')
            [CompletionResult]::new('set', 'set', [CompletionResultType]::ParameterValue, 'Change one setting')
            [CompletionResult]::new('unset', 'unset', [CompletionResultType]::ParameterValue, 'Reset one setting to its default')
            [CompletionResult]::new('path', 'path', [CompletionResultType]::ParameterValue, 'Print the path of the configuration file')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;config;list' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;config;get' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'ferry;config;set' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'ferry;config;unset' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
        'ferry;config;path' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;config;help' {
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'Show every setting')
            [CompletionResult]::new('get', 'get', [CompletionResultType]::ParameterValue, 'Show one setting')
            [CompletionResult]::new('set', 'set', [CompletionResultType]::ParameterValue, 'Change one setting')
            [CompletionResult]::new('unset', 'unset', [CompletionResultType]::ParameterValue, 'Reset one setting to its default')
            [CompletionResult]::new('path', 'path', [CompletionResultType]::ParameterValue, 'Print the path of the configuration file')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;config;help;list' {
            break
        }
        'ferry;config;help;get' {
            break
        }
        'ferry;config;help;set' {
            break
        }
        'ferry;config;help;unset' {
            break
        }
        'ferry;config;help;path' {
            break
        }
        'ferry;config;help;help' {
            break
        }
        'ferry;identity' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('show', 'show', [CompletionResultType]::ParameterValue, 'Show the Node ID and where the key is stored')
            [CompletionResult]::new('export', 'export', [CompletionResultType]::ParameterValue, 'Write a backup of the key to a new file (keep it secret)')
            [CompletionResult]::new('storage', 'storage', [CompletionResultType]::ParameterValue, 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)')
            [CompletionResult]::new('import', 'import', [CompletionResultType]::ParameterValue, 'Replace this device''s key with one from a backup')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;identity;show' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;identity;export' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;identity;storage' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;identity;import' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('--force', '--force', [CompletionResultType]::ParameterName, 'Replace an existing, different key (it is kept as a `.bak` file)')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;identity;help' {
            [CompletionResult]::new('show', 'show', [CompletionResultType]::ParameterValue, 'Show the Node ID and where the key is stored')
            [CompletionResult]::new('export', 'export', [CompletionResultType]::ParameterValue, 'Write a backup of the key to a new file (keep it secret)')
            [CompletionResult]::new('storage', 'storage', [CompletionResultType]::ParameterValue, 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)')
            [CompletionResult]::new('import', 'import', [CompletionResultType]::ParameterValue, 'Replace this device''s key with one from a backup')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;identity;help;show' {
            break
        }
        'ferry;identity;help;export' {
            break
        }
        'ferry;identity;help;storage' {
            break
        }
        'ferry;identity;help;import' {
            break
        }
        'ferry;identity;help;help' {
            break
        }
        'ferry;doctor' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;completions' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;generate-docs' {
            [CompletionResult]::new('--lang', '--lang', [CompletionResultType]::ParameterName, 'Interface language for this run (`en`, `pl`); overrides the saved setting')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'ferry;help' {
            [CompletionResult]::new('send', 'send', [CompletionResultType]::ParameterValue, 'Send files and folders to a trusted or nearby device (`--to`), or share one file by a one-time TICKET')
            [CompletionResult]::new('localsend', 'localsend', [CompletionResultType]::ParameterValue, 'Send files to LocalSend apps (iPhone, iPad, Mac, …) and find them on the local network')
            [CompletionResult]::new('integrate', 'integrate', [CompletionResultType]::ParameterValue, 'Add "Send with Ferry" to the file managers of this user (Dolphin, Nautilus, Nemo; Linux)')
            [CompletionResult]::new('history', 'history', [CompletionResultType]::ParameterValue, 'Show finished transfers, newest first, or search them')
            [CompletionResult]::new('web', 'web', [CompletionResultType]::ParameterValue, 'Share files with a web browser on the local network and receive files from it, through a one-time link and QR code (the other device needs no app)')
            [CompletionResult]::new('text', 'text', [CompletionResultType]::ParameterValue, 'Send text or a link; the receiver copies it to the clipboard after accepting')
            [CompletionResult]::new('receive', 'receive', [CompletionResultType]::ParameterValue, 'Download a file shared by TICKET')
            [CompletionResult]::new('daemon', 'daemon', [CompletionResultType]::ParameterValue, 'Run in the background; trusted devices with Auto-Accept are accepted automatically')
            [CompletionResult]::new('tui', 'tui', [CompletionResultType]::ParameterValue, 'Interactive terminal user interface')
            [CompletionResult]::new('ticket', 'ticket', [CompletionResultType]::ParameterValue, 'Show this device''s TICKET (text and QR code) for pairing')
            [CompletionResult]::new('discover', 'discover', [CompletionResultType]::ParameterValue, 'Search for Ferry devices on the local network (own UDP broadcast, no mDNS)')
            [CompletionResult]::new('pair', 'pair', [CompletionResultType]::ParameterValue, 'Pair with a nearby device (name or Node ID from `ferry discover`); the other side confirms and both show the same code')
            [CompletionResult]::new('pairing', 'pairing', [CompletionResultType]::ParameterValue, 'Let unknown devices send pairing requests for a few minutes, although `paired-only` is on (also for a running daemon or TUI); `--close` ends it')
            [CompletionResult]::new('guide', 'guide', [CompletionResultType]::ParameterValue, 'Short answers to the usual questions: sending, pairing, devices without Ferry, iPhone and Mac, local or internet, devices that do not show up, privacy')
            [CompletionResult]::new('name', 'name', [CompletionResultType]::ParameterValue, 'Show or change the name of this device visible to others')
            [CompletionResult]::new('language', 'language', [CompletionResultType]::ParameterValue, 'Show or change the interface language (`en`, `pl`)')
            [CompletionResult]::new('network', 'network', [CompletionResultType]::ParameterValue, 'Show or change the network mode: `local` (no external servers, default), `internet` (also through the free public iroh relays, for devices in different networks), `relay` (also through your own relays) or `relay-only` (only through your own relays, never directly)')
            [CompletionResult]::new('peer', 'peer', [CompletionResultType]::ParameterValue, 'Manage trusted devices')
            [CompletionResult]::new('config', 'config', [CompletionResultType]::ParameterValue, 'Show or change settings (`ferry config list`, `get`, `set`, `unset`, `path`)')
            [CompletionResult]::new('identity', 'identity', [CompletionResultType]::ParameterValue, 'Show, back up or restore this device''s identity (its Node ID key)')
            [CompletionResult]::new('doctor', 'doctor', [CompletionResultType]::ParameterValue, 'Network and configuration diagnostics')
            [CompletionResult]::new('completions', 'completions', [CompletionResultType]::ParameterValue, 'Print the shell completion script, e.g. `ferry completions bash > ~/.local/share/bash-completion/completions/ferry` (packages install it already)')
            [CompletionResult]::new('generate-docs', 'generate-docs', [CompletionResultType]::ParameterValue, 'Write the completion scripts and manual pages to `DIR/completions` and `DIR/man` (for packaging)')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'ferry;help;send' {
            break
        }
        'ferry;help;localsend' {
            [CompletionResult]::new('scan', 'scan', [CompletionResultType]::ParameterValue, 'List the LocalSend apps on the local network. Announces this device''s name to them once, so they answer')
            [CompletionResult]::new('send', 'send', [CompletionResultType]::ParameterValue, 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name')
            [CompletionResult]::new('text', 'text', [CompletionResultType]::ParameterValue, 'Send a text or a link to a LocalSend app, which shows it as a message')
            break
        }
        'ferry;help;localsend;scan' {
            break
        }
        'ferry;help;localsend;send' {
            break
        }
        'ferry;help;localsend;text' {
            break
        }
        'ferry;help;integrate' {
            break
        }
        'ferry;help;history' {
            break
        }
        'ferry;help;web' {
            break
        }
        'ferry;help;text' {
            break
        }
        'ferry;help;receive' {
            break
        }
        'ferry;help;daemon' {
            break
        }
        'ferry;help;tui' {
            break
        }
        'ferry;help;ticket' {
            break
        }
        'ferry;help;discover' {
            break
        }
        'ferry;help;pair' {
            break
        }
        'ferry;help;pairing' {
            break
        }
        'ferry;help;guide' {
            break
        }
        'ferry;help;name' {
            break
        }
        'ferry;help;language' {
            break
        }
        'ferry;help;network' {
            break
        }
        'ferry;help;peer' {
            [CompletionResult]::new('add', 'add', [CompletionResultType]::ParameterValue, 'Add a device by TICKET or Node ID and send it a pairing request')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'List trusted devices')
            [CompletionResult]::new('edit', 'edit', [CompletionResultType]::ParameterValue, 'Change the alias or Auto-Accept setting of a device')
            [CompletionResult]::new('verify', 'verify', [CompletionResultType]::ParameterValue, 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match')
            [CompletionResult]::new('remove', 'remove', [CompletionResultType]::ParameterValue, 'Remove a trusted device')
            break
        }
        'ferry;help;peer;add' {
            break
        }
        'ferry;help;peer;list' {
            break
        }
        'ferry;help;peer;edit' {
            break
        }
        'ferry;help;peer;verify' {
            break
        }
        'ferry;help;peer;remove' {
            break
        }
        'ferry;help;config' {
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'Show every setting')
            [CompletionResult]::new('get', 'get', [CompletionResultType]::ParameterValue, 'Show one setting')
            [CompletionResult]::new('set', 'set', [CompletionResultType]::ParameterValue, 'Change one setting')
            [CompletionResult]::new('unset', 'unset', [CompletionResultType]::ParameterValue, 'Reset one setting to its default')
            [CompletionResult]::new('path', 'path', [CompletionResultType]::ParameterValue, 'Print the path of the configuration file')
            break
        }
        'ferry;help;config;list' {
            break
        }
        'ferry;help;config;get' {
            break
        }
        'ferry;help;config;set' {
            break
        }
        'ferry;help;config;unset' {
            break
        }
        'ferry;help;config;path' {
            break
        }
        'ferry;help;identity' {
            [CompletionResult]::new('show', 'show', [CompletionResultType]::ParameterValue, 'Show the Node ID and where the key is stored')
            [CompletionResult]::new('export', 'export', [CompletionResultType]::ParameterValue, 'Write a backup of the key to a new file (keep it secret)')
            [CompletionResult]::new('storage', 'storage', [CompletionResultType]::ParameterValue, 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)')
            [CompletionResult]::new('import', 'import', [CompletionResultType]::ParameterValue, 'Replace this device''s key with one from a backup')
            break
        }
        'ferry;help;identity;show' {
            break
        }
        'ferry;help;identity;export' {
            break
        }
        'ferry;help;identity;storage' {
            break
        }
        'ferry;help;identity;import' {
            break
        }
        'ferry;help;doctor' {
            break
        }
        'ferry;help;completions' {
            break
        }
        'ferry;help;generate-docs' {
            break
        }
        'ferry;help;help' {
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
