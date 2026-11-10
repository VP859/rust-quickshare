# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_ferry_global_optspecs
    string join \n lang= h/help
end

function __fish_ferry_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_ferry_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_ferry_using_subcommand
    set -l cmd (__fish_ferry_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c ferry -n "__fish_ferry_needs_command" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_needs_command" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "send" -d 'Send files and folders to a trusted or nearby device (`--to`), or share one file by a one-time TICKET'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "localsend" -d 'Send files to LocalSend apps (iPhone, iPad, Mac, …) and find them on the local network'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "integrate" -d 'Add "Send with Ferry" to the file managers of this user (Dolphin, Nautilus, Nemo; Linux)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "history" -d 'Show finished transfers, newest first, or search them'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "web" -d 'Share files with a web browser on the local network and receive files from it, through a one-time link and QR code (the other device needs no app)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "text" -d 'Send text or a link; the receiver copies it to the clipboard after accepting'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "receive" -d 'Download a file shared by TICKET'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "daemon" -d 'Run in the background; trusted devices with Auto-Accept are accepted automatically'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "tui" -d 'Interactive terminal user interface'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "ticket" -d 'Show this device\'s TICKET (text and QR code) for pairing'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "discover" -d 'Search for Ferry devices on the local network (own UDP broadcast, no mDNS)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "pair" -d 'Pair with a nearby device (name or Node ID from `ferry discover`); the other side confirms and both show the same code'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "pairing" -d 'Let unknown devices send pairing requests for a few minutes, although `paired-only` is on (also for a running daemon or TUI); `--close` ends it'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "guide" -d 'Short answers to the usual questions: sending, pairing, devices without Ferry, iPhone and Mac, local or internet, devices that do not show up, privacy'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "name" -d 'Show or change the name of this device visible to others'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "language" -d 'Show or change the interface language (`en`, `pl`)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "network" -d 'Show or change the network mode: `local` (no external servers, default), `internet` (also through the free public iroh relays, for devices in different networks), `relay` (also through your own relays) or `relay-only` (only through your own relays, never directly)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "peer" -d 'Manage trusted devices'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "config" -d 'Show or change settings (`ferry config list`, `get`, `set`, `unset`, `path`)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "identity" -d 'Show, back up or restore this device\'s identity (its Node ID key)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "doctor" -d 'Network and configuration diagnostics'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "completions" -d 'Print the shell completion script, e.g. `ferry completions bash > ~/.local/share/bash-completion/completions/ferry` (packages install it already)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "generate-docs" -d 'Write the completion scripts and manual pages to `DIR/completions` and `DIR/man` (for packaging)'
complete -c ferry -n "__fish_ferry_needs_command" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand send" -s t -l to -d 'Alias, nearby device name, Node ID or TICKET of the receiver (it must run `ferry daemon`, the TUI or the GUI). Repeat it to send to several devices at once' -r
complete -c ferry -n "__fish_ferry_using_subcommand send" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand send" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and not __fish_seen_subcommand_from scan send help" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and not __fish_seen_subcommand_from scan send help" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and not __fish_seen_subcommand_from scan send help" -f -a "scan" -d 'List the LocalSend apps on the local network. Announces this device\'s name to them once, so they answer'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and not __fish_seen_subcommand_from scan send help" -f -a "send" -d 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and not __fish_seen_subcommand_from scan send help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from scan" -s t -l timeout -d 'Seconds to wait for answers' -r
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from scan" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from scan" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from send" -s t -l to -d 'Name or IP address (`192.168.1.20` or `192.168.1.20:53317`) of the LocalSend app, as listed by `ferry localsend scan`' -r
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from send" -l pin -d 'PIN, for apps that require one' -r
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from send" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from send" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from help" -f -a "scan" -d 'List the LocalSend apps on the local network. Announces this device\'s name to them once, so they answer'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from help" -f -a "send" -d 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name'
complete -c ferry -n "__fish_ferry_using_subcommand localsend; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand integrate" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand integrate" -l remove -d 'Remove the entries again'
complete -c ferry -n "__fish_ferry_using_subcommand integrate" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand history" -s n -l limit -d 'How many transfers to show' -r
complete -c ferry -n "__fish_ferry_using_subcommand history" -l kind -d 'Show only texts and links (never their content), or only files' -r -f -a "files\t'Files and folders only'
texts\t'Texts and links only'"
complete -c ferry -n "__fish_ferry_using_subcommand history" -l send-again -d 'Send the file of transfer N of the list (as numbered) again to the same Ferry device' -r
complete -c ferry -n "__fish_ferry_using_subcommand history" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand history" -l clear -d 'Delete the history'
complete -c ferry -n "__fish_ferry_using_subcommand history" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c ferry -n "__fish_ferry_using_subcommand web" -l minutes -d 'Minutes until the link stops working' -r
complete -c ferry -n "__fish_ferry_using_subcommand web" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand web" -l no-receive -d 'Do not let the browser send files here'
complete -c ferry -n "__fish_ferry_using_subcommand web" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand text" -s t -l to -d 'Alias, nearby device name, Node ID or TICKET of the receiver' -r
complete -c ferry -n "__fish_ferry_using_subcommand text" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand text" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand receive" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand receive" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand daemon" -s d -l downloads-dir -d 'Folder for received files (default: the saved setting or the download folder)' -r -F
complete -c ferry -n "__fish_ferry_using_subcommand daemon" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand daemon" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand tui" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand tui" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand ticket" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand ticket" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand discover" -s t -l timeout -d 'Search duration in seconds' -r
complete -c ferry -n "__fish_ferry_using_subcommand discover" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand discover" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand pair" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand pair" -l auto-accept -d 'Enable Auto-Accept for the paired device right away'
complete -c ferry -n "__fish_ferry_using_subcommand pair" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand pairing" -s m -l minutes -d 'How long pairing stays open, in minutes (at most 30)' -r
complete -c ferry -n "__fish_ferry_using_subcommand pairing" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand pairing" -l close -d 'Close pairing now'
complete -c ferry -n "__fish_ferry_using_subcommand pairing" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand guide" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand guide" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand name" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand name" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand language" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand language" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand network" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand network" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and not __fish_seen_subcommand_from add list edit verify remove help" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand peer; and not __fish_seen_subcommand_from add list edit verify remove help" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and not __fish_seen_subcommand_from add list edit verify remove help" -f -a "add" -d 'Add a device by TICKET or Node ID and send it a pairing request'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and not __fish_seen_subcommand_from add list edit verify remove help" -f -a "list" -d 'List trusted devices'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and not __fish_seen_subcommand_from add list edit verify remove help" -f -a "edit" -d 'Change the alias or Auto-Accept setting of a device'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and not __fish_seen_subcommand_from add list edit verify remove help" -f -a "verify" -d 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and not __fish_seen_subcommand_from add list edit verify remove help" -f -a "remove" -d 'Remove a trusted device'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and not __fish_seen_subcommand_from add list edit verify remove help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from add" -s a -l alias -d 'Name for the device' -r
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from add" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from add" -l auto-accept -d 'Accept files from this device without asking'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from add" -l no-pair -d 'Do not send a pairing request (trust only on this side)'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from add" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from list" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from edit" -s a -l alias -d 'New alias' -r
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from edit" -l auto-accept -d 'New Auto-Accept setting' -r -f -a "true\t''
false\t''"
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from edit" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from edit" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from verify" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from verify" -l confirm -d 'The numbers are equal: mark the device as verified'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from verify" -l unconfirm -d 'Remove the verified mark'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from verify" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from remove" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from remove" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from help" -f -a "add" -d 'Add a device by TICKET or Node ID and send it a pairing request'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from help" -f -a "list" -d 'List trusted devices'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from help" -f -a "edit" -d 'Change the alias or Auto-Accept setting of a device'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from help" -f -a "verify" -d 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from help" -f -a "remove" -d 'Remove a trusted device'
complete -c ferry -n "__fish_ferry_using_subcommand peer; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path help" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path help" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path help" -f -a "list" -d 'Show every setting'
complete -c ferry -n "__fish_ferry_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path help" -f -a "get" -d 'Show one setting'
complete -c ferry -n "__fish_ferry_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path help" -f -a "set" -d 'Change one setting'
complete -c ferry -n "__fish_ferry_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path help" -f -a "unset" -d 'Reset one setting to its default'
complete -c ferry -n "__fish_ferry_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path help" -f -a "path" -d 'Print the path of the configuration file'
complete -c ferry -n "__fish_ferry_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from list" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from get" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from get" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from set" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from set" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from unset" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from unset" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from path" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from path" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from help" -f -a "list" -d 'Show every setting'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from help" -f -a "get" -d 'Show one setting'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from help" -f -a "set" -d 'Change one setting'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from help" -f -a "unset" -d 'Reset one setting to its default'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from help" -f -a "path" -d 'Print the path of the configuration file'
complete -c ferry -n "__fish_ferry_using_subcommand config; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and not __fish_seen_subcommand_from show export storage import help" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand identity; and not __fish_seen_subcommand_from show export storage import help" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and not __fish_seen_subcommand_from show export storage import help" -f -a "show" -d 'Show the Node ID and where the key is stored'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and not __fish_seen_subcommand_from show export storage import help" -f -a "export" -d 'Write a backup of the key to a new file (keep it secret)'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and not __fish_seen_subcommand_from show export storage import help" -f -a "storage" -d 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and not __fish_seen_subcommand_from show export storage import help" -f -a "import" -d 'Replace this device\'s key with one from a backup'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and not __fish_seen_subcommand_from show export storage import help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from show" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from export" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from export" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from storage" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from storage" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from import" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from import" -l force -d 'Replace an existing, different key (it is kept as a `.bak` file)'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from import" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from help" -f -a "show" -d 'Show the Node ID and where the key is stored'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from help" -f -a "export" -d 'Write a backup of the key to a new file (keep it secret)'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from help" -f -a "storage" -d 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from help" -f -a "import" -d 'Replace this device\'s key with one from a backup'
complete -c ferry -n "__fish_ferry_using_subcommand identity; and __fish_seen_subcommand_from help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand doctor" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand doctor" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand completions" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand completions" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand generate-docs" -l lang -d 'Interface language for this run (`en`, `pl`); overrides the saved setting' -r
complete -c ferry -n "__fish_ferry_using_subcommand generate-docs" -s h -l help -d 'Print help'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "send" -d 'Send files and folders to a trusted or nearby device (`--to`), or share one file by a one-time TICKET'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "localsend" -d 'Send files to LocalSend apps (iPhone, iPad, Mac, …) and find them on the local network'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "integrate" -d 'Add "Send with Ferry" to the file managers of this user (Dolphin, Nautilus, Nemo; Linux)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "history" -d 'Show finished transfers, newest first, or search them'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "web" -d 'Share files with a web browser on the local network and receive files from it, through a one-time link and QR code (the other device needs no app)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "text" -d 'Send text or a link; the receiver copies it to the clipboard after accepting'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "receive" -d 'Download a file shared by TICKET'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "daemon" -d 'Run in the background; trusted devices with Auto-Accept are accepted automatically'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "tui" -d 'Interactive terminal user interface'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "ticket" -d 'Show this device\'s TICKET (text and QR code) for pairing'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "discover" -d 'Search for Ferry devices on the local network (own UDP broadcast, no mDNS)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "pair" -d 'Pair with a nearby device (name or Node ID from `ferry discover`); the other side confirms and both show the same code'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "pairing" -d 'Let unknown devices send pairing requests for a few minutes, although `paired-only` is on (also for a running daemon or TUI); `--close` ends it'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "guide" -d 'Short answers to the usual questions: sending, pairing, devices without Ferry, iPhone and Mac, local or internet, devices that do not show up, privacy'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "name" -d 'Show or change the name of this device visible to others'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "language" -d 'Show or change the interface language (`en`, `pl`)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "network" -d 'Show or change the network mode: `local` (no external servers, default), `internet` (also through the free public iroh relays, for devices in different networks), `relay` (also through your own relays) or `relay-only` (only through your own relays, never directly)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "peer" -d 'Manage trusted devices'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "config" -d 'Show or change settings (`ferry config list`, `get`, `set`, `unset`, `path`)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "identity" -d 'Show, back up or restore this device\'s identity (its Node ID key)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "doctor" -d 'Network and configuration diagnostics'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "completions" -d 'Print the shell completion script, e.g. `ferry completions bash > ~/.local/share/bash-completion/completions/ferry` (packages install it already)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "generate-docs" -d 'Write the completion scripts and manual pages to `DIR/completions` and `DIR/man` (for packaging)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and not __fish_seen_subcommand_from send localsend integrate history web text receive daemon tui ticket discover pair pairing guide name language network peer config identity doctor completions generate-docs help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from localsend" -f -a "scan" -d 'List the LocalSend apps on the local network. Announces this device\'s name to them once, so they answer'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from localsend" -f -a "send" -d 'Send files and folders to a LocalSend app. Its name is not verified: anyone on the network can announce any name'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from peer" -f -a "add" -d 'Add a device by TICKET or Node ID and send it a pairing request'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from peer" -f -a "list" -d 'List trusted devices'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from peer" -f -a "edit" -d 'Change the alias or Auto-Accept setting of a device'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from peer" -f -a "verify" -d 'Show the safety number shared with a trusted device, to compare it with the other device; `--confirm` records that both match'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from peer" -f -a "remove" -d 'Remove a trusted device'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from config" -f -a "list" -d 'Show every setting'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from config" -f -a "get" -d 'Show one setting'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from config" -f -a "set" -d 'Change one setting'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from config" -f -a "unset" -d 'Reset one setting to its default'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from config" -f -a "path" -d 'Print the path of the configuration file'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from identity" -f -a "show" -d 'Show the Node ID and where the key is stored'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from identity" -f -a "export" -d 'Write a backup of the key to a new file (keep it secret)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from identity" -f -a "storage" -d 'Show or change where the key is kept: `file` (default) or `system` (Secret Service / KWallet / GNOME Keyring on Linux, Credential Manager on Windows)'
complete -c ferry -n "__fish_ferry_using_subcommand help; and __fish_seen_subcommand_from identity" -f -a "import" -d 'Replace this device\'s key with one from a backup'
