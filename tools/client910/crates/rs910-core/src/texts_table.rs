//! The localised text table (see the `texts` module): one row per message in
//! the enum's order, one column per language in the order of its `COLUMNS`
//! (English, German, French, Portuguese, Latin American Spanish).
//! `None` is an absent cell. Data only.

/// The text rows.
pub(crate) static TABLE: [[Option<&str>; 5]; 176] = [
    // DebugConsoleInfo
    [Some("This is the developer console. To close, press the ALT-`, ALT-² or ALT-§ keys on your keyboard."), Some("Das ist die Entwicklerkonsole. Zum Schließen, die Tasten ALT+`, ALT+² oder ALT+§ drücken."), Some("Ceci est la console de développement. Pour la fermer, appuyez sur les touches ALT+`, ALT+² ou ALT+§."), Some("Este é o painel de controle do desenvolvedor. Para fechar, pressione ALT-`, ALT-² ou ALT-§."), Some("Esta es la consola de desarrolador. Para cerrarla, pulsa las teclas ALT-`, ALT-² or ALT-§ en tu teclado.")],
    // DebugConsoleError
    [Some("There was an error executing the command."), Some("Es gab einen Fehler beim Ausführen des Befehls."), Some("Une erreur s'est produite lors de l'exécution de la commande."), Some("Houve um erro quando o comando foi executado."), Some("Se produjo un error al ejecutar el comando.")],
    // DeveloperConsoleShortcutInfo
    [Some("The developer console can be accessed with ALT-`, ALT-§ or ALT-²."), Some("Die Entwicklerkonsole kann mit ALT+`, ALT+§ oder ALT+² aktiviert werden."), Some("La console de développement est accessible grâce aux touches ALT+`, ALT+§ ou ALT+²."), Some("O painel de controle do desenvolvedor pode ser acessado com ALT-`, ALT-§ ou ALT-²."), Some("Puedes acceder a la consola de desarrollador con ALT-`, ALT-§ o ALT-².")],
    // DebugConsoleUnknownCommand
    [Some("Unknown developer command: "), Some("Unbekannter Befehl: "), Some("Commande inconnue : "), Some("Comando desconhecido: "), Some("Comando desconocido: ")],
    // Cancel
    [Some("Cancel"), Some("Abbrechen"), Some("Annuler"), Some("Cancelar"), Some("Cancelar")],
    // NoNamePlayername
    [Some("#Player"), Some("#Spieler"), Some("#Joueur"), Some("#Jogador"), Some("#Jugador")],
    // MembersDesc
    [Some("Login to a members' server to use this object."), Some("Du musst auf einer Mitglieder-Welt sein, um diesen Gegenstand zu benutzen."), Some("Connectez-vous à un serveur d'abonnés pour utiliser cet objet."), Some("Acesse um servidor para membros para usar este objeto."), Some("Accede a un servidor para miembros para usar este objeto.")],
    // SwapNoteAtBank
    [Some("Swap this note at any bank for the equivalent item."), Some("Dieses Zertifikat kann in einer Bank entsprechend eingetauscht werden."), Some("Échangez ce reçu contre l'objet correspondant dans la banque de votre choix."), Some("Vá a qualquer banco para trocar esta nota pelo objeto equivalente."), Some("Cambia este vale en cualquier banco por el objeto equivalente.")],
    // LentItemReturn
    [Some("Discard"), Some("Ablegen"), Some("Jeter"), Some("Descartar"), Some("Descartar")],
    // BoughtItemDiscard
    [Some("Discard"), Some("Ablegen"), Some("Jeter"), Some("Descartar"), Some("Descartar")],
    // ShardCombinePrefix
    [Some("Combine "), Some("Kombiniere "), Some("Vous pouvez combiner "), Some("Você pode trocar "), Some("Puedes combinar ")],
    // ShardCombineSuffix
    [Some(" shards of this type to receive a "), Some(" dieser Fragmente, um folgenden Gegenstand herzustellen: "), Some(" de ces fragments pour obtenir l'objet suivant : "), Some(" desses fragmentos pelo seguinte objeto: "), Some(" de estos fragmentos para obtener el siguiente objeto: ")],
    // ShardItemCombine
    [Some("Combine"), Some("Kombinieren"), Some("Combiner"), Some("Combinar"), Some("Combinar")],
    // Take
    [Some("Take"), Some("Nehmen"), Some("Prendre"), Some("Pegar"), Some("Agarrar")],
    // Drop
    [Some("Drop"), Some("Fallen lassen"), Some("Poser"), Some("Largar"), Some("Dejar")],
    // Ok
    [Some("Ok"), Some("Okay"), Some("OK"), Some("Ok"), Some("OK")],
    // Select
    [Some("Select"), Some("Auswählen"), Some("Sélectionner"), Some("Selecionar"), Some("Seleccionar")],
    // Continue
    [Some("Continue"), Some("Weiter"), Some("Continuer"), Some("Continuar"), Some("Continuar")],
    // InvalidPlayerName
    [Some("Invalid player name."), Some("Unzulässiger Charaktername!"), Some("Nom de joueur incorrect."), Some("Nome de jogador inválido."), Some("Nombre de jugador no válido.")],
    // YouCantReportYourself
    [Some("You can't report yourself!"), Some("Du kannst dich nicht selbst melden!"), Some("Vous ne pouvez pas vous signaler vous-même !"), Some("Você não pode denunciar a si próprio!"), Some("¡No te puedes denunciar a ti mismo!")],
    // YouAlreadySentASnapshot1
    [Some("You have sent too many abuse reports today! Do not abuse this system!"), Some("Du hast heute schon zu viele Regelverstöße gemeldet! Missbrauch das System nicht!"), Some("Vous avez signalé trop d’abus pour aujourd’hui. N’abusez pas de ce système !"), Some("Você já denunciou abuso muitas vezes hoje. Não abuse do sistema!"), Some("¡Ya has denunciado demasiadas infracciones hoy! ¡No abuses del sistema!")],
    // YouCannotReportStaffForImpersonation1
    [Some("You cannot report that person for Staff Impersonation, they are Jagex Staff."), Some("Diese Person ist ein Jagex-Mitarbeiter!"), Some("Cette personne est un membre du personnel de Jagex, vous ne pouvez pas la signaler pour abus d'identité."), Some("Você não pode denunciar essa pessoa por tentar se passar por um membro da equipe Jagex, pois ela faz parte da equipe."), Some("Esa persona es miembro del personal de Jagex, no puedes denunciarla por suplantación de identidad.")],
    // YouCannotReportStaffForImpersonation2
    [Some("You can spot a Jagex moderator by the gold crown next to their name."), Some("Jagex-Mitarbeiter haben eine goldene Krone neben ihrem Namen."), Some("Vous pouvez reconnaître les modérateurs Jagex à la couronne dorée en regard de leur nom."), Some("Os moderadores da Jagex são identificados por uma coroa dourada ao lado de seu nome."), Some("Los moderadores de Jagex tienen una corona dorada a un lado del nombre.")],
    // YouCannotReportStaffForImpersonation3
    [Some("You can report that person under a different rule."), Some("Diese Person kann bezüglich einer anderen Regel gemeldet werden."), Some("Vous pouvez signaler cette personne pour une autre infraction aux règles."), Some("Você pode denunciar essa pessoa por outro tipo de infração."), Some("Puedes denunciar a esa persona por otro tipo de infracción.")],
    // AbuseReportReceived
    [Some("Thank-you, your abuse report has been received."), Some("Vielen Dank, deine Meldung ist bei uns eingegangen."), Some("Merci, nous avons bien reçu votre rapport d'abus."), Some("Obrigado. Sua denúncia de abuso foi recebida."), Some("Gracias, hemos recibido tu denuncia.")],
    // UnableToSendSnapshotBusy
    [Some("Unable to send abuse report - system busy."), Some("Meldung konnte nicht gesendet werden - Systeme überlastet"), Some("Impossible de signaler un abus - Erreur système"), Some("Sistema ocupado. Não foi possível enviar sua denúncia de abuso."), Some("Sistema ocupado. No ha sido posible enviar tu denuncia.")],
    // InvalidName
    [Some("Invalid name"), Some("Unzulässiger Name!"), Some("Nom incorrect"), Some("Nome inválido"), Some("Nombre no válido")],
    // UseMembersServerItem
    [Some("To use this item please login to a members' server."), Some("Du musst auf einer Mitglieder-Welt sein, um diesen Gegenstand zu benutzen."), Some("Veuillez vous connecter à un serveur d'abonnés pour utiliser cet objet."), Some("Acesse um servidor para membros para usar este objeto."), Some("Accede a un servidor para miembros para usar este objeto.")],
    // UseMembersServerLocation
    [Some("To interact with this please login to a members' server."), Some("Logg dich auf einer Mitglieder-Welt ein, um damit zu interagieren."), Some("Veuillez vous connecter à un serveur d'abonnés pour cette interaction."), Some("Para interagir, acesse um servidor para membros."), Some("Para interactuar, accede a un servidor para miembros.")],
    // NothingInterestingHappens
    [Some("Nothing interesting happens."), Some("Nichts Interessantes passiert."), Some("Il ne se passe rien d'intéressant."), Some("Nada de interessante acontece."), Some("No sucede nada interesante.")],
    // ICantReachThat
    [Some("You can't reach that."), Some("Da kommst du nicht hin."), Some("Vous ne pouvez pas l'atteindre."), Some("Você não consegue alcançar isso."), Some("No puedes alcanzar eso.")],
    // InvalidTeleport
    [Some("Invalid teleport!"), Some("Unzulässiger Teleport!"), Some("Téléportation non valide !"), Some("Teleporte inválido!"), Some("¡Teletransporte no válido!")],
    // UseMembersServerCoord
    [Some("To go here you must login to a members' server."), Some("Du musst auf einer Mitglieder-Welt sein, um dort hinzukommen."), Some("Vous devez vous connecter à un serveur d'abonnés pour aller à cet endroit."), Some("Para entrar aqui, acesse um servidor para membros."), Some("Para entrar aquí, debes acceder a un servidor para miembros.")],
    // UnableToAddFriendSystem
    [Some("Unable to add friend - system busy."), Some("Der Freund konnte nicht hinzugefügt werden, das System ist derzeit ausgelastet."), Some("Impossible d'ajouter un ami - système occupé."), Some("Não foi possível adicionar o amigo. O sistema está ocupado."), Some("Sistema ocupado. No es posible añadir a un amigo.")],
    // UnableToAddFriendExists
    [Some("Unable to add friend - unknown player."), Some("Spieler konnte nicht hinzugefügt werden - Spieler unbekannt."), Some("Impossible d'ajouter l'ami - joueur inconnu."), Some("Não foi possível adicionar esse amigo - jogador desconhecido."), Some("Jugador desconocido. No es posible añadir a ese amigo.")],
    // UnableToAddIgnoreSystem
    [Some("Unable to add name - system busy."), Some("Der Name konnte nicht hinzugefügt werden, das System ist derzeit ausgelastet."), Some("Impossible d'ajouter un nom - système occupé."), Some("Não foi possível adicionar o nome. O sistema está ocupado."), Some("Sistema ocupado. No es posible añadir el nombre.")],
    // UnableToAddIgnoreExists
    [Some("Unable to add name - unknown player."), Some("Name konnte nicht hinzugefügt werden - Spieler unbekannt."), Some("Impossible d'ajouter le nom - joueur inconnu."), Some("Não foi possível adicionar esse nome - jogador desconhecido."), Some("Jugador desconocido. No es posible añadir el nombre.")],
    // FriendlistFullMembers
    [Some("Your friends list is full (400 names maximum)"), Some("Deine Freunde-Liste ist voll, du hast das Maximum von 400 erreicht."), Some("Votre liste d'amis est pleine (400 noms maximum)."), Some("Sua lista de amigos está cheia. O limite é de 400 nomes."), Some("Tu lista de amigos está llena. El límite es de 400 amigos.")],
    // FriendlistFull
    [Some("Your friends list is full (200 names maximum)"), Some("Deine Freunde-Liste ist voll, du hast das Maximum von 200 erreicht."), Some("Votre liste d'amis est pleine (200 noms maximum)."), Some("Sua lista de amigos está cheia. O limite é de 200 nomes."), Some("Tu lista de amigos está llena. El límite es de 200 amigos.")],
    // UnableToDeleteFriend
    [Some("Unable to delete friend - system busy."), Some("Der Freund konnte nicht entfernt werden, das System ist derzeit ausgelastet."), Some("Impossible de supprimer un ami - système occupé."), Some("Não foi possível excluir o amigo. O sistema está ocupado."), Some("Servidor ocupado. No es posible borrar al amigo.")],
    // UnableToDeleteIgnore
    [Some("Unable to delete name - system busy."), Some("Name konnte nicht gelöscht werden - Systemfehler."), Some("Impossible d'effacer le nom - système occupé."), Some("Não foi possível deletar o nome - sistema ocupado."), Some("Sistema ocupado. No es posible borrar el nombre.")],
    // UnableToSendMessageBusy
    [Some("Unable to send message - system busy."), Some("Deine Nachricht konnte nicht verschickt werden, das System ist derzeit ausgelastet."), Some("Impossible d'envoyer un message - système occupé."), Some("Não foi possível enviar a mensagem. O sistema está ocupado."), Some("Sistema ocupado. No es posible enviar el mensaje.")],
    // UnableToSendMessageUnavailable1
    [Some("Unable to send message - player unavailable."), Some("Deine Nachricht konnte nicht verschickt werden,"), Some("Impossible d'envoyer un message - joueur indisponible."), Some("Não foi possível enviar a mensagem. O jogador não está disponível."), Some("No es posible enviar el mensaje, el jugador no está disponible.")],
    // UnableToSendMessageUnavailable2
    [None, Some("der Spieler ist momentan nicht verfügbar."), None, None, None],
    // UnableToSendMessageNotFriend1
    [Some("Unable to send message - player not on your friends list."), Some("Nachricht kann nicht geschickt werden,"), Some("Impossible d'envoyer un message - joueur non inclus dans votre liste d'amis."), Some("Não foi possível enviar a mensagem. O jogador não está na sua lista de amigos."), Some("No es posible enviar el mensaje. El jugador no está en tu lista de amigos.")],
    // UnableToSendMessageNotFriend2
    [None, Some("Spieler nicht auf deiner Freunde-Liste."), None, None, None],
    // UnableToSendMessagePasswordA
    [Some("You appear to be telling someone your password - please don't!"), Some("Willst du jemandem dein Passwort verraten? Das darfst du nicht! Falls das"), Some("Il semble que vous révéliez votre mot de passe à quelqu'un - ne faites jamais ça !"), Some("Parece que você está revelando sua senha a alguém. Não faça isso!"), Some("Parece que le estás revelando a alguien tu contraseña. ¡No debes hacerlo!")],
    // UnableToSendMessagePasswordB
    [Some("If you are not, please change your password to something more obscure!"), Some("nicht der Fall ist, ändere dein Passwort zu einem ungewöhnlicheren Begriff!"), Some("Si ce n'est pas le cas, remplacez votre mot de passe par une formule moins évidente !"), Some("Caso não esteja, altere sua senha para algo mais obscuro!"), Some("¡Si no es así, cambia tu contraseña por una menos evidente!")],
    // UnableToSendMessageNoDisplayname1
    [Some("Unable to send message - set your display name first by logging into the game."), Some("Nachricht konnte nicht gesendet werden.  Bitte richte erst deinen Charakternamen ein, "), Some("Impossible d'envoyer le message - enregistrez un nom de personnage en vous connectant au jeu."), Some("Não é possível enviar a mensagem. Defina um nome de personagem primeiro, fazendo login no jogo."), Some("No es posible enviar el mensaje. Registra primero un nombre de personaje conectándote al juego.")],
    // UnableToSendMessageNoDisplayname2
    [None, Some("indem du dich ins Spiel einloggst."), None, None, None],
    // SnapshotBufferEmpty1
    [Some("For that rule you can only report players who have spoken or traded recently."), Some("Mit dieser Option können nur Spieler gemeldet werden,"), Some("Cette règle n'est invocable que pour les discussions ou échanges récents."), Some("Para essa regra, você só pode denunciar jogadores com quem tenha falado ou negociado recentemente."), Some("Sólo puedes denunciar por esa regla a jugadores que hayan hablado o comerciado recientemente.")],
    // SnapshotBufferEmpty2
    [None, Some("die kürzlich gesprochen oder gehandelt haben."), None, None, None],
    // NameDialogNotFound
    [Some("That player is offline, or has privacy mode enabled."), Some("Dieser Spieler ist offline oder hat den Privatsphären-Modus aktiviert."), Some("Ce joueur est déconnecté ou en mode privé."), Some("O jogador está offline ou está com o modo de privacidade ativado."), Some("Este jugador está desconectado o activó el modo de privacidad.")],
    // UnableToSendMessageQuickChat1
    [Some("You cannot send a quick chat message to a player on this world at this time."), Some("Einem Spieler auf dieser Welt können derzeit keine Direktchat-Nachrichten"), Some("Impossible d'envoyer un message rapide à un joueur de ce serveur à l'heure actuelle."), Some("Você não pode enviar uma mensagem de papo rápido para um jogador neste mundo neste momento."), Some("En estos momentos no puedes enviar un mensaje rápido de chat a un jugador en este mundo.")],
    // UnableToSendMessageQuickChat2
    [None, Some("geschickt werden."), None, None, None],
    // UnableToSendMessageQuickChatWorld1
    [Some("This player is on a quick chat world and cannot receive your message."), Some("Der Spieler kann auf einer Direktchat-Welt keine Nachrichten empfangen."), Some("Ce joueur est sur un serveur à messagerie rapide et ne peut pas recevoir votre message."), Some("Este jogador não pode receber sua mensagem porque está em um mundo de papo rápido."), Some("Este jugador no puede recibir su mensaje porque está en un mundo de chat rápido.")],
    // ChatDisabled
    [Some("Chat disabled"), Some("Deaktiviert"), Some("Messagerie désactivée"), Some("Bate-papo desativado"), Some("Chat desactivado")],
    // Under13FriendsChatPrefix
    [Some("friends_chat"), Some("friends_chat"), Some("friends_chat"), Some("friends_chat"), Some("friends_chat")],
    // UnableToSendMessageNotInFriendsChat
    [Some("You are not currently in a friends chat channel."), Some("Du befindest dich derzeit nicht in einem Freundes-Chatraum."), Some("Vous ne faites pas partie d'un canal de discussion."), Some("No momento, você não está no bate-papo entre amigos."), Some("Actualmente no estás en un canal de chat entre amigos.")],
    // UnableToSendMessageFriendsChatTooLowRank
    [Some("You are not allowed to talk in this friends chat channel."), Some("Du darfst in diesem Freundes-Chatraum nicht reden."), Some("Vous n'êtes pas autorisé à parler dans ce canal de discussion."), Some("Você não está autorizado a falar neste bate-papo entre amigos."), Some("No estás autorizado a hablar en este canal de chat entre amigos.")],
    // UnableToSendMessageFriendsChatError
    [Some("Error sending message to friends chat - please try again later!"), Some("Fehler beim Versenden der Nachricht - bitte versuch es später erneut."), Some("Erreur lors de l'envoi de ce message – veuillez réessayer ultérieurement !"), Some("Erro ao enviar mensagem para bate-papo entre amigos - tente novamente mais tarde!"), Some("Se ha producido un error al enviar un mensaje al chat entre amigos, por favor, inténtalo más tarde.")],
    // FriendsChatStillInChannel
    [Some("Please wait until you are logged out of your previous channel."), Some("Bitte warte, bis du den vorherigen Chatraum verlassen hast."), Some("Veuillez attendre d'être déconnecté(e) de votre canal précédent."), Some("Aguarde até se desconectar do canal anterior."), Some("Por favor, espera hasta haberte desconectado del anterior chat.")],
    // FriendsChatNotInChannel
    [Some("You are not currently in a channel."), Some("Du befindest dich derzeit nicht in einem Chatraum."), Some("Vous n'êtes dans aucun canal à l'heure actuelle."), Some("No momento você não está em um canal."), Some("En este momento no estás en un canal.")],
    // FriendsChatAttemptingJoin
    [Some("Attempting to join channel..."), Some("Chatraum wird betreten..."), Some("Tentative de connexion au canal..."), Some("Tentando acessar canal..."), Some("Intentando acceder a un canal...")],
    // FriendsChatSendingLeaveReq
    [Some("Sending request to leave channel..."), Some("Chatraum wird verlassen..."), Some("Envoi de la demande de sortie du canal..."), Some("Enviando solicitação para sair do canal..."), Some("Enviando solicitud para abandonar el canal...")],
    // FriendsChatJoinInProgress
    [Some("Already attempting to join a channel - please wait..."), Some("Du versuchst bereits, einem Chatraum beizutreten - bitte warte."), Some("Tentative de connexion au canal déjà en cours - veuillez patienter..."), Some("Já há uma tentativa de entrar em um canal. Aguarde..."), Some("Ya estás intentando unirte a un canal. Por favor, espera...")],
    // FriendsChatLeaveInProgress
    [Some("Leave request already in progress - please wait..."), Some("Du versuchst bereits, einen Chatraum zu verlassen - bitte warte."), Some("Demande de sortie déjà effectuée - veuillez patienter..."), Some("Solicitação de saída já em andamento. Aguarde..."), Some("La salida del canal está procesándose. Por favor, espera...")],
    // FriendsChatInvalidName
    [Some("Invalid channel name entered!"), Some("Ungültiger Chatraum-Name angegeben."), Some("Nom de canal incorrect !"), Some("Nome de canal inválido!"), Some("¡Nombre de canal no valido!")],
    // FriendsChatNotAvailable
    [Some("Unable to join friends chat at this time - please try again later!"), Some("Freundes-Chatraum kann nicht betreten werden - bitte versuch es später erneut."), Some("Vous ne pouvez pas rejoindre ce canal de discussion pour le moment - veuillez réessayer ultérieurement !"), Some("Não foi possível entrar no bate-papo entre amigos - tente novamente mais tarde!"), Some("Ahora mismo no es posible unirse al chat entre amigos. ¡Por favor, inténtalo más tarde!")],
    // FriendsChatJoinSuccessA
    [Some("Now talking in friends chat channel "), Some("Freundes-Chatraum: "), Some("Vous participez actuellement au canal de discussion : "), Some("Falando agora no bate-papo entre amigos: "), Some("Hablando ahora en el chat entre amigos: ")],
    // FriendsChatJoinSuccessAUnder13
    [Some("Now talking in friends chat channel of player: "), Some("Freundes-Chat dieses Spielers beigetreten: "), Some("Vous participez actuellement au canal de discussion du joueur : "), Some("Falando agora no bate-papo entre amigos do jogador: "), Some("Hablando ahora en el canal de chat entre amigos del jugador: ")],
    // FriendsChatJoinError
    [Some("Error joining friends chat channel - please try again later!"), Some("Fehler beim Betreten des Freundes-Chatraums - bitte versuch es später erneut."), Some("Erreur lors de la connexion au canal de discussion - veuillez réessayer ultérieurement !"), Some("Erro ao participar do bate-papo entre amigos - tente novamente mais tarde!"), Some("Se ha producido un error al acceder al canal de chat entre amigos. ¡Por favor, inténtalo más tarde!")],
    // FriendsChatJoinAttackBlocked
    [Some("You are temporarily blocked from joining channels - please try again later!"), Some("Du darfst derzeit keine Chaträume betreten - bitte versuch es später."), Some("Vous êtes temporairement exclu des canaux - veuillez réessayer ultérieurement."), Some("Você está temporariamente impedido de entrar em canais. Tente novamente mais tarde!"), Some("De momento tienes bloqueado el acceso a los canales chat. ¡Inténtalo de nuevo más tarde!")],
    // FriendsChatJoinNotExist
    [Some("The channel you tried to join does not exist."), Some("Der von dir gewünschte Chatraum existiert nicht."), Some("Le canal que vous essayez de rejoindre n'existe pas."), Some("O canal que você tentou acessar não existe."), Some("El canal al que intentas unirte no existe.")],
    // FriendsChatJoinRoomFull
    [Some("The channel you tried to join is currently full."), Some("Der von dir gewünschte Chatraum ist derzeit überfüllt."), Some("Le canal que vous essayez de rejoindre est plein."), Some("O canal que você tentou acessar está cheio no momento."), Some("El canal al que intentas unirte está lleno en estos momentos.")],
    // FriendsChatJoinLowRank
    [Some("You do not have a high enough rank to join this friends chat channel."), Some("Dein Rang reicht nicht aus, um diesen Freundes-Chatraum zu betreten."), Some("Votre rang n'est pas assez élevé pour rejoindre ce canal de discussion."), Some("Você não tem uma classificação alta o suficiente para participar deste bate-papo entre amigos."), Some("No tienes rango suficiente para unirte a este canal de chat entre amigos.")],
    // FriendsChatJoinBanned
    [Some("You are temporarily banned from this friends chat channel."), Some("Du wurdest temporär von diesem Freundes-Chatraum gesperrt."), Some("Vous avez été exclu temporairement de ce canal de discussion."), Some("Você foi temporariamente banido deste bate-papo entre amigos."), Some("Tienes bloqueado temporalmente el acceso a este chat entre amigos.")],
    // FriendsChatJoinIgnoreList
    [Some("You are not allowed to join this user's friends chat channel."), Some("Du darfst den Freundes-Chatraum dieses Benutzers nicht betreten."), Some("Vous n'êtes pas autorisé à rejoindre le canal de discussion de cet utilisateur."), Some("Você não pode entrar nesse bate-papo entre amigos deste usuário."), Some("No tienes permiso para acceder al canal de chat entre amigos de este usuario.")],
    // FriendsChatUserJoined
    [Some(" joined the channel."), Some(" hat den Chatraum betreten."), Some(" a rejoint le canal."), Some(" entrou no canal."), Some(" se ha unido al canal.")],
    // FriendsChatUserLeft
    [Some(" left the channel."), Some(" hat den Chatraum verlassen."), Some(" a quitté le canal."), Some(" saiu do canal."), Some(" ha abandonado el canal.")],
    // FriendsChatUserKicked
    [Some(" was kicked from the channel."), Some(" wurde aus dem Chatraum rausgeworfen."), Some(" a été expulsé du canal."), Some(" foi expulso do canal."), Some(" ha sido expulsado del canal.")],
    // FriendsChatLeaveKicked
    [Some("You have been kicked from the channel."), Some("Du wurdest aus dem Chatraum rausgeworfen."), Some("Vous avez été expulsé du canal."), Some("Você foi expulso do canal."), Some("Se te ha expulsado del canal.")],
    // FriendsChatLeaveRemoved
    [Some("You have been removed from this channel."), Some("Du wurdest aus dem Chatraum entfernt."), Some("Vous avez été supprimé de ce canal."), Some("Você foi retirado desse canal."), Some("Se te ha eliminado de este canal.")],
    // FriendsChatLeaveDefault
    [Some("You have left the channel."), Some("Du hast den Chatraum verlassen."), Some("Vous avez quitté le canal."), Some("Você saiu do canal."), Some("Has salido del canal.")],
    // FriendsChatEnabledA
    [Some("Your friends chat channel has now been enabled!"), Some("Dein Freundes-Chat ist jetzt eingeschaltet."), Some("Votre canal de discussion est maintenant activé !"), Some("O seu bate-papo entre amigos foi ativado!"), Some("¡Tu canal de chat entre amigos está activado!")],
    // FriendsChatEnabledB
    [Some("Join your channel by clicking 'Join Chat' and typing: "), Some("Klick auf 'Betreten' und gib ein: "), Some("Pour rejoindre votre canal, cliquez sur « Participer » et entrez : "), Some("Para entrar no seu canal, clique em \"Acessar bate-papo\" e digite: "), Some("Para entrar en tu canal, haz clic sobre 'Participar' e introduce: ")],
    // FriendsChatDisabled
    [Some("Your friends chat channel has now been disabled!"), Some("Dein Freundes-Chat ist jetzt ausgeschaltet."), Some("Votre canal de discussion est maintenant désactivé !"), Some("O seu bate-papo entre amigos foi desativado!"), Some("¡Tu canal de chat entre amigos ha sido desactivado!")],
    // FriendsChatKickLowRank
    [Some("You do not have permission to kick users in this channel."), Some("Du darfst keine Benutzer aus diesem Chatraum rauswerfen."), Some("Vous n'êtes pas autorisé à expulser des utilisateurs de ce canal."), Some("Você não tem permissão para expulsar usuários neste canal."), Some("No tienes autorización para expulsar a usuarios de este canal.")],
    // FriendsChatKickUserHigher
    [Some("You do not have permission to kick this user."), Some("Du darfst diesen Benutzer nicht rauswerfen."), Some("Vous n'êtes pas autorisé à expulser cet utilisateur."), Some("Você não tem permissão para expulsar este usuário."), Some("No tienes autorización para expulsar a este usuario.")],
    // FriendsChatKickNotFound
    [Some("That user is not in this channel."), Some("Dieser Benutzer befindet sich nicht in diesem Chatraum."), Some("Cet utilisateur n'est pas dans ce canal."), Some("Esse usuário não está no canal."), Some("Ese usuario no está en este canal.")],
    // FriendsChatKickSuccess
    [Some("Your request to kick/ban this user was successful."), Some("Der Rauswurf/die Sperrung war erfolgreich."), Some("Votre demande d'exclusion de ce joueur a été acceptée."), Some("Seu pedido para expulsar/suspender este jogador foi bem sucedido."), Some("Tu petición de expulsar/suspender a este usuario ha sido aceptada.")],
    // FriendsChatKickSuccessReset
    [Some("Your request to refresh this user's temporary ban was successful."), Some("Die Verlängerung der temporären Sperrung dieses Spielers war erfolgreich."), Some("Le renouvellement d'exclusion temporaire de ce joueur a été accepté."), Some("Seu pedido para reiniciar a suspensão temporária deste jogador foi bem sucedido."), Some("Tu petición de prolongar la suspensión temporal de este usuario ha sido aceptada.")],
    // MutedTemporary
    [Some("You have been temporarily muted due to breaking a rule."), Some("Aufgrund eines Regelverstoßes wurdest du vorübergehend stumm geschaltet."), Some("La messagerie instantanée a été temporairement bloquée suite à une infraction."), Some("Você foi temporariamente vetado por ter violado uma regra."), Some("Se te ha vetado temporalmente por haber violado una regla.")],
    // MutedTemporaryTimeA
    [Some("This mute will remain for a further "), Some("Diese Stummschaltung gilt für weitere "), Some("Votre accès restera bloqué encore "), Some("Este veto permanecerá por mais "), Some("Este veto permancereá activo todavía durante ")],
    // MutedTemporaryTimeB
    [Some(" days."), Some(" Tage."), Some(" jours."), Some(" dias."), Some(" días.")],
    // MutedTemporaryOneDay
    [Some("You will be un-muted within 24 hours."), Some("Du wirst innerhalb der nächsten 24 Stunden wieder sprechen können."), Some("Vous aurez à nouveau accès à la messagerie instantanée dans 24 heures."), Some("O veto será retirado dentro de 24 horas."), Some("Tu veto se retirará dentro de las próximas 24 horas.")],
    // MutedPrevent
    [Some("To prevent further mutes please read the rules."), Some("Um eine erneute Stummschaltung zu verhindern, lies bitte die Regeln."), Some("Pour éviter un nouveau blocage, lisez le règlement."), Some("Para evitar outros vetos, leia as regras."), Some("Para evitar otro veto, consulta el reglamento.")],
    // MutedPermanent
    [Some("You have been permanently muted due to breaking a rule."), Some("Du wurdest permanent stumm geschaltet, da du gegen eine Regel verstoßen hast."), Some("L'accès à la messagerie instantanée vous a définitivement été retiré suite à une infraction."), Some("Você foi permanentemente vetado por ter violado uma regra."), Some("Se te ha vetado permanentemente por haber violado una regla.")],
    // Loading
    [Some("Loading - please wait."), Some("Ladevorgang - bitte warte."), Some("Chargement en cours. Veuillez patienter."), Some("Carregando. Aguarde."), Some("Cargando. Por favor, espera.")],
    // Profiling
    [Some("Profiling..."), Some("Profiling..."), Some("Profilage..."), Some("Definindo perfil..."), Some("Obteniendo perfil...")],
    // ConnectionLost
    [Some("Connection lost."), Some("Verbindung abgebrochen."), Some("Connexion perdue."), Some("Conexão perdida."), Some("Conexión perdida.")],
    // AttemptToReestablish
    [Some("Please wait - attempting to reestablish."), Some("Bitte warte - es wird versucht, die Verbindung wiederherzustellen."), Some("Veuillez patienter - tentative de rétablissement."), Some("Tentando reestabelecer conexão. Aguarde."), Some("Estamos intentando restablecer la conexión. Por favor, espera.")],
    // CheckingForUpdates
    [Some("Checking for updates"), Some("Suche nach Updates"), Some("Vérification des mises à jour"), Some("Verificando atualizações"), Some("Buscando actualizaciones")],
    // DownloadingUpdates
    [Some("Fetching Updates"), Some("Lade Update"), Some("Chargement des MAJ"), Some("Carregando atualizações"), Some("Cargando actualizaciones")],
    // LoadConfig
    [Some("Loading config - "), Some("Lade Konfiguration - "), Some("Chargement des fichiers config - "), Some("Carregando config - "), Some("Cargando configuración - ")],
    // LoadedConfig
    [Some("Loaded config"), Some("Konfig geladen."), Some("Fichiers config chargés"), Some("Config carregada"), Some("Configuración cargada")],
    // LoadSprites
    [Some("Loading sprites - "), Some("Lade Sprites - "), Some("Chargement des sprites - "), Some("Carregando sprites - "), Some("Cargando sprites - ")],
    // LoadedSprites
    [Some("Loaded sprites"), Some("Sprites geladen."), Some("Sprites chargés"), Some("Sprites carregados"), Some("Sprites cargados")],
    // LoadWordpack
    [Some("Loading wordpack - "), Some("Lade Wordpack - "), Some("Chargement du module texte - "), Some("Carregando pacote de palavras - "), Some("Cargando el módulo de texto - ")],
    // LoadedWordpack
    [Some("Loaded wordpack"), Some("Wordpack geladen."), Some("Module texte chargé"), Some("Pacote de palavras carregado"), Some("Módulo de texto cargado")],
    // LoadInterfaces
    [Some("Loading interfaces - "), Some("Lade Benutzeroberfläche - "), Some("Chargement des interfaces - "), Some("Carregando interfaces - "), Some("Cargando interfaces - ")],
    // LoadedInterfaces
    [Some("Loaded interfaces"), Some("Benutzeroberfläche geladen."), Some("Interfaces chargées"), Some("Interfaces carregadas"), Some("Interfaces cargadas")],
    // LoadInterfaceScripts
    [Some("Loading interface scripts - "), Some("Lade Interface-Skripte - "), Some("Chargement des interfaces - "), Some("Carregando scripts de interface - "), Some("Cargando guión de interfaz - ")],
    // LoadedInterfaceScripts
    [Some("Loaded interface scripts"), Some("Interface-Skripte geladen"), Some("Interfaces chargées"), Some("Script de interface carregados"), Some("Guiones de interfaz cargados")],
    // LoadAdditionalFonts
    [Some("Loading additional fonts - "), Some("Lade Zusatzschriftarten - "), Some("Chargement de polices secondaires - "), Some("Carregando fontes adicionais - "), Some("Cargando fuentes adicionales - ")],
    // LoadedAdditionalFonts
    [Some("Loaded additional fonts"), Some("Zusatzschriftarten geladen"), Some("Polices secondaires chargées"), Some("Fontes adicionais carregadas"), Some("Fuentes adicionales cargadas")],
    // LoadWorldMap
    [Some("Loading world map - "), Some("Lade Weltkarte - "), Some("Chargement de la mappemonde - "), Some("Carregando mapa-múndi - "), Some("Cargando mapamundi - ")],
    // LoadedWorldMap
    [Some("Loaded world map"), Some("Weltkarte geladen"), Some("Mappemonde chargée"), Some("Mapa-múndi carregado"), Some("Mapamundi cargado")],
    // LoadWorldList
    [Some("Loading world list data"), Some("Lade Liste der Welten"), Some("Chargement de la liste des serveurs"), Some("Carregando dados da lista de mundos"), Some("Cargando datos de la lista de mundos")],
    // LoadedWorldList
    [Some("Loaded world list data"), Some("Liste der Welten geladen"), Some("Liste des serveurs chargée"), Some("Dados da lista de mundos carregados"), Some("Datos de la lista de mundos cargados")],
    // LoadedClientVariables
    [Some("Loaded client variable data"), Some("Client-Variablen geladen"), Some("Variables du client chargées"), Some("As variáveis do sistema foram carregadas"), Some("Variables de cliente cargadas")],
    // LoadingEllipsis
    [Some("Loading..."), Some("Lade..."), Some("Chargement en cours..."), Some("Carregando..."), Some("Cargando...")],
    // SnapshotPleaseclose1
    [Some("Please close the interface you have open before using 'Report Abuse'."), Some("Bitte schließ die momentan geöffnete Benutzeroberfläche,"), Some("Fermez l'interface que vous avez ouverte avant d'utiliser le bouton « Signaler un abus »."), Some("Feche a interface aberta antes de usar o recurso \"Denunciar abuso\"."), Some("Cierra la interfaz que tienes abierta antes de usar el botón 'Denunciar abuso'.")],
    // SnapshotPleaseclose2
    [None, Some("bevor du die Option 'Regelverstoß melden' benutzt."), None, None, None],
    // Systemupdate
    [Some("System update in: "), Some("System-Update in: "), Some("Mise à jour système dans : "), Some("Atualização do sistema em: "), Some("Actualización del sistema en: ")],
    // FriendLogin
    [Some(" has logged in."), Some(" loggt sich ein."), Some(" s'est connecté."), Some(" entrou no jogo."), Some(" se ha conectado.")],
    // FriendLogout
    [Some(" has logged out."), Some(" loggt sich aus."), Some(" s'est déconnecté."), Some(" saiu do jogo."), Some(" se ha desconectado.")],
    // UnableToFind
    [Some("Unable to find "), Some("Spieler kann nicht gefunden werden: "), Some("Impossible de trouver "), Some("Não foi possível encontrar "), Some("No es posible encontrar a ")],
    // Use
    [Some("Use"), Some("Benutzen"), Some("Utiliser"), Some("Usar"), Some("Usar")],
    // Examine
    [Some("Examine"), Some("Untersuchen"), Some("Examiner"), Some("Examinar"), Some("Examinar")],
    // Attack
    [Some("Attack"), Some("Angreifen"), Some("Attaquer"), Some("Atacar"), Some("Atacar")],
    // ChooseOption
    [Some("Choose Option"), Some("Wähl eine Option"), Some("Choisir une option"), Some("Selecionar opção"), Some("Seleccionar opción")],
    // MoreOptions
    [Some(" more options"), Some(" weitere Optionen"), Some(" autres options"), Some(" mais opções"), Some(" más opciones")],
    // WalkHere
    [Some("Walk here"), Some("Hierhin gehen"), Some("Atteindre"), Some("Caminhar para cá"), Some("Venir acá")],
    // FaceHere
    [Some("Face here"), Some("Hierhin drehen"), Some("Regarder dans cette direction"), Some("Virar para cá"), Some("Girar hacia acá")],
    // Level
    [Some("level: "), Some("Stufe: "), Some("niveau "), Some("nível: "), Some("nivel: ")],
    // Skill
    [Some("skill: "), Some("Fertigkeit: "), Some("compétence "), Some("habilidade: "), Some("habilidad: ")],
    // Rating
    [Some("rating: "), Some("Kampfstufe: "), Some("classement "), Some("qualificação: "), Some("clasificación: ")],
    // PleaseWait
    [Some("Please wait..."), Some("Bitte warte..."), Some("Veuillez attendre"), Some("Aguarde..."), Some("Por favor, espera...")],
    // Close
    [Some("Close"), Some("Bitte schließ die momentan geöffnete Benutzeroberfläche,"), Some("Fermez l'interface que vous avez ouverte avant d'utiliser le bouton « Signaler un abus »."), Some("Feche a interface aberta antes de usar o recurso \"Denunciar abuso\"."), Some("Cierra la interfaz que tienes abierta antes de usar el botón 'Denunciar abuso'.")],
    // MenuSeparator
    [Some(" "), Some(": "), Some(" "), Some(" "), Some(" ")],
    // Million
    [Some("M"), Some("M"), Some("M"), Some("M"), Some("M")],
    // MillionShort
    [Some("M"), Some("M"), Some("M"), Some("M"), Some("M")],
    // Thousand
    [Some("K"), Some("T"), Some("K"), Some("K"), Some("K")],
    // ThousandShort
    [Some("K"), Some("T"), Some("K"), Some("K"), Some("K")],
    // From
    [Some("From"), Some("Von:"), Some("De"), Some("De"), Some("De")],
    // SelfLabel
    [Some("Self"), Some("Mich"), Some("Moi"), Some("Eu"), Some("Mí")],
    // FriendListDupe
    [Some(" is already on your friends list."), Some(" steht bereits auf deiner Freunde-Liste!"), Some(" est déjà dans votre liste d'amis."), Some(" já está na sua lista de amigos."), Some(" ya está en tu lista de amigos.")],
    // IgnoreListFullMembers
    [Some("Your ignore list is full. Max of 400 users."), Some("Deine Ignorieren-Liste ist voll, du kannst nur 400 Spieler darauf eintragen."), Some("Votre liste noire est pleine (400 noms maximum)."), Some("Sua lista de ignorados está cheia. O limite é de 400 usuários."), Some("Tu lista de jugadores ignorados está llena, el límite es de 400.")],
    // IgnoreListFull
    [Some("Your ignore list is full. Max of 100 users."), Some("Deine Ignorieren-Liste ist voll, du kannst nur 100 Spieler darauf eintragen."), Some("Votre liste noire est pleine (100 noms maximum)."), Some("Sua lista de ignorados está cheia. O limite é de 100 usuários."), Some("Tu lista de jugadores ignorados está llena, el límite es de 100.")],
    // IgnoreListDupe
    [Some(" is already on your ignore list."), Some(" steht bereits auf deiner Ignorieren-Liste!"), Some(" est déjà dans votre liste noire."), Some(" já está na sua lista de ignorados."), Some(" ya está en tu lista de ignorados.")],
    // FriendCantAddSelf
    [Some("You can't add yourself to your own friends list."), Some("Du kannst dich nicht auf deine eigene Freunde-Liste setzen!"), Some("Vous ne pouvez pas ajouter votre nom à votre liste d'amis."), Some("Você não pode adicionar a si próprio à sua lista de amigos."), Some("No puedes añadirte a tu propia lista de amigos.")],
    // IgnoreCantAddSelf
    [Some("You can't add yourself to your own ignore list."), Some("Du kannst dich nicht auf deine eigene Ignorieren-Liste setzen!"), Some("Vous ne pouvez pas ajouter votre nom à votre liste noire."), Some("Você não pode adicionar a si próprio à sua lista de ignorados."), Some("No puedes añadirte a tu propia lista de ignorados.")],
    // FriendlistTimedSave
    [Some("Changes will take effect on your friends chat in the next 60 seconds."), Some("Die Änderungen am Freundes-Chat werden innerhalb von 60 Sekunden übernommen."), Some("Les modifications seront apportées à votre canal de discussion dans les 60 prochaines secondes."), Some("As mudanças acontecerão em seu bate-papo entre amigos nos próximos 60 segundos."), Some("Los cambios en tu chat de amigos se realizarán en los próximos 60 segundos.")],
    // RemoveIgnore1
    [Some("Please remove "), Some("Bitte entferne "), Some("Veuillez commencer par supprimer "), Some("Remova "), Some("Elimina primero a ")],
    // RemoveIgnore2
    [Some(" from your ignore list first."), Some(" zuerst von deiner Ignorieren-Liste!"), Some(" de votre liste noire."), Some(" da sua lista de ignorados primeiro."), Some(" de tu lista de ignorados.")],
    // RemoveFriend1
    [Some("Please remove "), Some("Bitte entferne "), Some("Veuillez commencer par supprimer "), Some("Remova "), Some("Elimina primero a ")],
    // RemoveFriend2
    [Some(" from your friends list first."), Some(" zuerst von deiner Freunde-Liste!"), Some(" de votre liste d'amis."), Some(" da sua lista de amigos primeiro."), Some(" de tu lista de amigos.")],
    // Chatcol0
    [Some("yellow:"), Some("gelb:"), Some("jaune:"), Some("amarelo:"), Some("amarillo:")],
    // Chatcol1
    [Some("red:"), Some("rot:"), Some("rouge:"), Some("vermelho:"), Some("rojo:")],
    // Chatcol2
    [Some("green:"), Some("grün:"), Some("vert:"), Some("verde:"), Some("verde:")],
    // Chatcol3
    [Some("cyan:"), Some("blaugrün:"), Some("cyan:"), Some("ciano:"), Some("cian:")],
    // Chatcol4
    [Some("purple:"), Some("lila:"), Some("violet:"), Some("roxo:"), Some("violeta:")],
    // Chatcol5
    [Some("white:"), Some("weiss:"), Some("blanc:"), Some("branco:"), Some("blanco:")],
    // Chatcol6
    [Some("flash1:"), Some("blinken1:"), Some("clignotant1:"), Some("flash1:"), Some("parpadeante1:")],
    // Chatcol7
    [Some("flash2:"), Some("blinken2:"), Some("clignotant2:"), Some("flash2:"), Some("parpadeante2:")],
    // Chatcol8
    [Some("flash3:"), Some("blinken3:"), Some("clignotant3:"), Some("flash3:"), Some("parpadeante3:")],
    // Chatcol9
    [Some("glow1:"), Some("leuchten1:"), Some("brillant1:"), Some("brilho1:"), Some("brillante1:")],
    // Chatcol10
    [Some("glow2:"), Some("leuchten2:"), Some("brillant2:"), Some("brilho2:"), Some("brillante2:")],
    // Chatcol11
    [Some("glow3:"), Some("leuchten3:"), Some("brillant3:"), Some("brilho3:"), Some("brillante3:")],
    // Chateffect1
    [Some("wave:"), Some("welle:"), Some("ondulation:"), Some("onda:"), Some("onda:")],
    // Chateffect2
    [Some("wave2:"), Some("welle2:"), Some("ondulation2:"), Some("onda2:"), Some("onda2:")],
    // Chateffect3
    [Some("shake:"), Some("schütteln:"), Some("tremblement:"), Some("tremor:"), Some("temblor:")],
    // Chateffect4
    [Some("scroll:"), Some("scrollen:"), Some("déroulement:"), Some("rolagem:"), Some("desplazar:")],
    // Chateffect5
    [Some("slide:"), Some("gleiten:"), Some("glissement:"), Some("deslizamento:"), Some("deslizar:")],
    // UnknownFriendDisplaynamePlaceholder
    [Some("Friend"), Some("Freund"), Some("Ami"), Some("Amigo"), Some("Amigo")],
];
