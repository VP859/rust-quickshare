
use builtin;
use str;

set edit:completion:arg-completer[ferry] = {|@words|
    fn spaces {|n|
        builtin:repeat $n ' ' | str:join ''
    }
    fn cand {|text desc|
        edit:complex-candidate $text &display=$text' '(spaces (- 14 (wcswidth $text)))$desc
    }
    var command = 'ferry'
    for word $words[1..-1] {
        if (str:has-prefix $word '-') {
            break
        }
        set command = $command';'$word
    }
    var completions = [
        &'ferry'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
            cand send 'Send files and folders to a trusted or nearby device (`--to`), or share one file by a one-time TICKET'
            cand localsend 'Send files to LocalSend apps (iPhone, iPad, Mac, …) and find them on the local network'
            cand integrate 'Add "Send with Ferry" to the file managers of this user (Dolphin, Nautilus, Nemo; Linux)'
            cand history 'Show finished transfers, newest first, or search them'
            cand web 'Share files with a web browser on the local network and receive files from it, through a one-time link and QR code (the other device needs no app)'
            cand text 'Send text or a link; the receiver copies it to the clipboard after accepting'
            cand receive 'Download a file shared by TICKET'
            cand daemon 'Run in the background; trusted devices with Auto-Accept are accepted automatically'
            cand tui 'Interactive terminal user interface'
            cand ticket 'Show this device''s TICKET (text and QR code) for pairing'
            cand discover 'Search for Ferry devices on the local network (own UDP broadcast, no mDNS)'
            cand pair 'Pair with a nearby device (name or Node ID from `ferry discover`); the other side confirms and both show the same code'
            cand pairing 'Let unknown devices send pairing requests for a few minutes, although `paired-only` is on (also for a running daemon or TUI); `--close` ends it'
            cand guide 'Short answers to the usual questions: sending, pairing, devices without Ferry, iPhone and Mac, local or internet, devices that do not show up, privacy'
            cand name 'Show or change the name of this device visible to others'
            cand language 'Show or change the interface language (`en`, `pl`)'
            cand network 'Show or change the network mode: `local` (no external servers, default), `internet` (also through the free public iroh relays, for devices in different networks), `relay` (also through your own relays) or `relay-only` (only through your own relays, never directly)'
            cand peer 'Manage trusted devices'
            cand config 'Show or change settings (`ferry config list`, `get`, `set`, `unset`, `path`)'
            cand identity 'Show, back up or restore this device''s identity (its Node ID key)'
            cand doctor 'Network and configuration diagnostics'
            cand completions 'Print the shell completion script, e.g. `ferry completions bash > ~/.local/share/bash-completion/completions/ferry` (packages install it already)'
            cand generate-docs 'Write the completion scripts and manual pages to `DIR/completions` and `DIR/man` (for packaging)'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;send'= {
            cand -t 'Alias, nearby device name, Node ID or TICKET of the receiver (it must run `ferry daemon`, the TUI or the GUI). Repeat it to send to several devices at once'
            cand --to 'Alias, nearby device name, Node ID or TICKET of the receiver (it must run `ferry daemon`, the TUI or the GUI). Repeat it to send to several devices at once'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;localsend'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
            cand scan 'List the LocalSend apps on the local network. Announces this device''s name to them once, so they answer'
            cand send 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;localsend;scan'= {
            cand -t 'Seconds to wait for answers'
            cand --timeout 'Seconds to wait for answers'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;localsend;send'= {
            cand -t 'Name or IP address (`192.168.1.20` or `192.168.1.20:53317`) of the LocalSend app, as listed by `ferry localsend scan`'
            cand --to 'Name or IP address (`192.168.1.20` or `192.168.1.20:53317`) of the LocalSend app, as listed by `ferry localsend scan`'
            cand --pin 'PIN, for apps that require one'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;localsend;help'= {
            cand scan 'List the LocalSend apps on the local network. Announces this device''s name to them once, so they answer'
            cand send 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;localsend;help;scan'= {
        }
        &'ferry;localsend;help;send'= {
        }
        &'ferry;localsend;help;help'= {
        }
        &'ferry;integrate'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand --remove 'Remove the entries again'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;history'= {
            cand -n 'How many transfers to show'
            cand --limit 'How many transfers to show'
            cand --kind 'Show only texts and links (never their content), or only files'
            cand --send-again 'Send the file of transfer N of the list (as numbered) again to the same Ferry device'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand --clear 'Delete the history'
            cand -h 'Print help (see more with ''--help'')'
            cand --help 'Print help (see more with ''--help'')'
        }
        &'ferry;web'= {
            cand --minutes 'Minutes until the link stops working'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand --no-receive 'Do not let the browser send files here'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;text'= {
            cand -t 'Alias, nearby device name, Node ID or TICKET of the receiver'
            cand --to 'Alias, nearby device name, Node ID or TICKET of the receiver'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;receive'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;daemon'= {
            cand -d 'Folder for received files (default: the saved setting or the download folder)'
            cand --downloads-dir 'Folder for received files (default: the saved setting or the download folder)'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;tui'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;ticket'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;discover'= {
            cand -t 'Search duration in seconds'
            cand --timeout 'Search duration in seconds'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;pair'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand --auto-accept 'Enable Auto-Accept for the paired device right away'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;pairing'= {
            cand -m 'How long pairing stays open, in minutes (at most 30)'
            cand --minutes 'How long pairing stays open, in minutes (at most 30)'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand --close 'Close pairing now'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;guide'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;name'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;language'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;network'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;peer'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
            cand add 'Add a device by TICKET or Node ID and send it a pairing request'
            cand list 'List trusted devices'
            cand edit 'Change the alias or Auto-Accept setting of a device'
            cand verify 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match'
            cand remove 'Remove a trusted device'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;peer;add'= {
            cand -a 'Name for the device'
            cand --alias 'Name for the device'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand --auto-accept 'Accept files from this device without asking'
            cand --no-pair 'Do not send a pairing request (trust only on this side)'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;peer;list'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;peer;edit'= {
            cand -a 'New alias'
            cand --alias 'New alias'
            cand --auto-accept 'New Auto-Accept setting'
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;peer;verify'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand --confirm 'The numbers are equal: mark the device as verified'
            cand --unconfirm 'Remove the verified mark'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;peer;remove'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;peer;help'= {
            cand add 'Add a device by TICKET or Node ID and send it a pairing request'
            cand list 'List trusted devices'
            cand edit 'Change the alias or Auto-Accept setting of a device'
            cand verify 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match'
            cand remove 'Remove a trusted device'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;peer;help;add'= {
        }
        &'ferry;peer;help;list'= {
        }
        &'ferry;peer;help;edit'= {
        }
        &'ferry;peer;help;verify'= {
        }
        &'ferry;peer;help;remove'= {
        }
        &'ferry;peer;help;help'= {
        }
        &'ferry;config'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'Show every setting'
            cand get 'Show one setting'
            cand set 'Change one setting'
            cand unset 'Reset one setting to its default'
            cand path 'Print the path of the configuration file'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;config;list'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;config;get'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help (see more with ''--help'')'
            cand --help 'Print help (see more with ''--help'')'
        }
        &'ferry;config;set'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help (see more with ''--help'')'
            cand --help 'Print help (see more with ''--help'')'
        }
        &'ferry;config;unset'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help (see more with ''--help'')'
            cand --help 'Print help (see more with ''--help'')'
        }
        &'ferry;config;path'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;config;help'= {
            cand list 'Show every setting'
            cand get 'Show one setting'
            cand set 'Change one setting'
            cand unset 'Reset one setting to its default'
            cand path 'Print the path of the configuration file'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;config;help;list'= {
        }
        &'ferry;config;help;get'= {
        }
        &'ferry;config;help;set'= {
        }
        &'ferry;config;help;unset'= {
        }
        &'ferry;config;help;path'= {
        }
        &'ferry;config;help;help'= {
        }
        &'ferry;identity'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
            cand show 'Show the Node ID and where the key is stored'
            cand export 'Write a backup of the key to a new file (keep it secret)'
            cand storage 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)'
            cand import 'Replace this device''s key with one from a backup'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;identity;show'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;identity;export'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;identity;storage'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;identity;import'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand --force 'Replace an existing, different key (it is kept as a `.bak` file)'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;identity;help'= {
            cand show 'Show the Node ID and where the key is stored'
            cand export 'Write a backup of the key to a new file (keep it secret)'
            cand storage 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)'
            cand import 'Replace this device''s key with one from a backup'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;identity;help;show'= {
        }
        &'ferry;identity;help;export'= {
        }
        &'ferry;identity;help;storage'= {
        }
        &'ferry;identity;help;import'= {
        }
        &'ferry;identity;help;help'= {
        }
        &'ferry;doctor'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;completions'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;generate-docs'= {
            cand --lang 'Interface language for this run (`en`, `pl`); overrides the saved setting'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'ferry;help'= {
            cand send 'Send files and folders to a trusted or nearby device (`--to`), or share one file by a one-time TICKET'
            cand localsend 'Send files to LocalSend apps (iPhone, iPad, Mac, …) and find them on the local network'
            cand integrate 'Add "Send with Ferry" to the file managers of this user (Dolphin, Nautilus, Nemo; Linux)'
            cand history 'Show finished transfers, newest first, or search them'
            cand web 'Share files with a web browser on the local network and receive files from it, through a one-time link and QR code (the other device needs no app)'
            cand text 'Send text or a link; the receiver copies it to the clipboard after accepting'
            cand receive 'Download a file shared by TICKET'
            cand daemon 'Run in the background; trusted devices with Auto-Accept are accepted automatically'
            cand tui 'Interactive terminal user interface'
            cand ticket 'Show this device''s TICKET (text and QR code) for pairing'
            cand discover 'Search for Ferry devices on the local network (own UDP broadcast, no mDNS)'
            cand pair 'Pair with a nearby device (name or Node ID from `ferry discover`); the other side confirms and both show the same code'
            cand pairing 'Let unknown devices send pairing requests for a few minutes, although `paired-only` is on (also for a running daemon or TUI); `--close` ends it'
            cand guide 'Short answers to the usual questions: sending, pairing, devices without Ferry, iPhone and Mac, local or internet, devices that do not show up, privacy'
            cand name 'Show or change the name of this device visible to others'
            cand language 'Show or change the interface language (`en`, `pl`)'
            cand network 'Show or change the network mode: `local` (no external servers, default), `internet` (also through the free public iroh relays, for devices in different networks), `relay` (also through your own relays) or `relay-only` (only through your own relays, never directly)'
            cand peer 'Manage trusted devices'
            cand config 'Show or change settings (`ferry config list`, `get`, `set`, `unset`, `path`)'
            cand identity 'Show, back up or restore this device''s identity (its Node ID key)'
            cand doctor 'Network and configuration diagnostics'
            cand completions 'Print the shell completion script, e.g. `ferry completions bash > ~/.local/share/bash-completion/completions/ferry` (packages install it already)'
            cand generate-docs 'Write the completion scripts and manual pages to `DIR/completions` and `DIR/man` (for packaging)'
            cand help 'Print this message or the help of the given subcommand(s)'
        }
        &'ferry;help;send'= {
        }
        &'ferry;help;localsend'= {
            cand scan 'List the LocalSend apps on the local network. Announces this device''s name to them once, so they answer'
            cand send 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name'
        }
        &'ferry;help;localsend;scan'= {
        }
        &'ferry;help;localsend;send'= {
        }
        &'ferry;help;integrate'= {
        }
        &'ferry;help;history'= {
        }
        &'ferry;help;web'= {
        }
        &'ferry;help;text'= {
        }
        &'ferry;help;receive'= {
        }
        &'ferry;help;daemon'= {
        }
        &'ferry;help;tui'= {
        }
        &'ferry;help;ticket'= {
        }
        &'ferry;help;discover'= {
        }
        &'ferry;help;pair'= {
        }
        &'ferry;help;pairing'= {
        }
        &'ferry;help;guide'= {
        }
        &'ferry;help;name'= {
        }
        &'ferry;help;language'= {
        }
        &'ferry;help;network'= {
        }
        &'ferry;help;peer'= {
            cand add 'Add a device by TICKET or Node ID and send it a pairing request'
            cand list 'List trusted devices'
            cand edit 'Change the alias or Auto-Accept setting of a device'
            cand verify 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match'
            cand remove 'Remove a trusted device'
        }
        &'ferry;help;peer;add'= {
        }
        &'ferry;help;peer;list'= {
        }
        &'ferry;help;peer;edit'= {
        }
        &'ferry;help;peer;verify'= {
        }
        &'ferry;help;peer;remove'= {
        }
        &'ferry;help;config'= {
            cand list 'Show every setting'
            cand get 'Show one setting'
            cand set 'Change one setting'
            cand unset 'Reset one setting to its default'
            cand path 'Print the path of the configuration file'
        }
        &'ferry;help;config;list'= {
        }
        &'ferry;help;config;get'= {
        }
        &'ferry;help;config;set'= {
        }
        &'ferry;help;config;unset'= {
        }
        &'ferry;help;config;path'= {
        }
        &'ferry;help;identity'= {
            cand show 'Show the Node ID and where the key is stored'
            cand export 'Write a backup of the key to a new file (keep it secret)'
            cand storage 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)'
            cand import 'Replace this device''s key with one from a backup'
        }
        &'ferry;help;identity;show'= {
        }
        &'ferry;help;identity;export'= {
        }
        &'ferry;help;identity;storage'= {
        }
        &'ferry;help;identity;import'= {
        }
        &'ferry;help;doctor'= {
        }
        &'ferry;help;completions'= {
        }
        &'ferry;help;generate-docs'= {
        }
        &'ferry;help;help'= {
        }
    ]
    $completions[$command]
}
