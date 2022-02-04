;************************************************;
; Convert LBA to CHS
; AX: LBA Address to convert
;
; absolute sector = (logical sector / sectors per track) + 1
; absolute head   = (logical sector / sectors per track) MOD number of heads
; absolute track  = logical sector / (sectors per track * number of heads)
;
;************************************************;
LBA2CHS:
	xor     dx, dx							; prepare dx:ax for operation
	div     WORD [bpbSectorsPerTrack]		; calculate
	inc     dl								; adjust for sector 0
	mov     BYTE [Sector], dl
	xor     dx, dx							; prepare dx:ax for operation
	div     WORD [bpbHeadsPerCylinder]		; calculate
	mov     BYTE [Head], dl
	mov     BYTE [Track], al
	ret

;************************************************;
; Converts a FAT Cluster to LBA.
; We have to substract 2 from the FAT cluster, because the first 2
; FAT clusters have a special purpose, and they have no
; corresponding data cluster in the file
;
; LBA = (FAT Cluster - 2) * sectors per cluster
;************************************************;
FATCluster2LBA:
    sub     ax, 0x0002                          ; zero base cluster number
    xor     cx, cx
    mov     cl, BYTE [bpbSectorsPerCluster]     ; convert byte to word
    mul     cx
    add     ax, WORD [DataSectorBeginning]      ; base data sector
    ret
    
;=============================================
; Input String is in register "si"
;=============================================
printline:
	mov al, [si]
	cmp al, 0
	je end_printline

	int 0x10
	inc si
	jmp printline

	end_printline:
ret

;======================================================
; Loads data from the disk
; dh: number of the sectors we want to read
; cl: number of the sector were we will start to read
;======================================================
disk_load:
	push dx

	mov		ah, 0x02					; BIOS read selector function
	mov		al, dh						; Number of the sector we want to read
	mov		ch, BYTE [Track]		    ; Track
	mov		cl, BYTE [Sector]			; Sector
	mov		dh, BYTE [Head]			    ; Head
	mov		dl, 0           			; Select the boot drive
	int		0x13						; BIOS interrupt that triggers the I/O

	jc		disk_error					; Error handling

	pop		dx
	cmp		dh, al						; Do we have read the amount of sectors that we have expected
	jne		disk_error
	ret

;=============================================
disk_error:
	mov ah, 0x0e
	mov si, disk_read_error_message
	call printline
	jmp $

;=========================================================
; Implementation of a simple FAT12 driver that is able
; to load the C kernel file from the file system
;=========================================================
LoadRootDirectory:
	; In the first step we calculate the size (number of sectors) 
	; of the root directory and store the value in the CX register
	; Calculation: 32 * bpbRootEntries / bpbBytesPerSector
	xor     cx, cx
	xor     dx, dx
	mov     ax, 0x0020                      ; 32 byte directory entry
	mul     WORD [bpbRootEntries]           ; total size of directory
	div     WORD [bpbBytesPerSector]        ; sectors used by directory
	xchg    ax, cx
          
	; In the next step we calculate the LBA address (number of the sector)
	; of the root directory and store the location in the AX register
	; AX holds afterwards an LBA address, which must be converted to a CHS address
	;
	; Calcuation: bpbNumberOfFATs * bpbSectorsPerFAT + bpbReservedSectors
	mov     al, BYTE [bpbNumberOfFATs]       ; Number of FATs
	mul     WORD [bpbSectorsPerFAT]          ; Number of sectors used by the FATs
	add     ax, WORD [bpbReservedSectors]    ; Add the boot sector (and reserved sectors, if available)

	; Calculate the address where the first cluster of data begins
	; Calculation: Root Directory Size (register AX) + (size of FATs + boot sector + reserved sectors [register CX])
	mov     WORD [DataSectorBeginning], ax   ; Size of the root directory
    add     WORD [DataSectorBeginning], cx	 ; FAT sectors + boot sector + reserved sectors

	; Convert the calculated LBA address (stored in AX) to a CHS address
	call	LBA2CHS

	; And finally we read the complete root directory into memory
	mov		bx, ROOTDIRECTORY_AND_FAT_OFFSET	; Load the Root Directory at 0x1000
	mov		dh, cl								; Load the number of sectors stored in CX
	call	disk_load							; Perform the I/O operation

	; Now we have to find our file in the Root Directory
	mov     cx, [bpbRootEntries]				; The number of root directory entries
	mov     di, ROOTDIRECTORY_AND_FAT_OFFSET    ; Address of the Root directory
    .Loop:
		push    cx
		mov     cx, 11					; We compare 11 characters (8.3 convention)
		mov     si, SecondStageName		; Compare against the kernel image name
		push    di
	rep  cmpsb							; Test for string match
		pop     di
		je      LOAD_FAT				; When we have a match, we load the FAT
		pop     cx
		add     di, 32					; When we don't have a match, we go to next root directory entry (+ 32 bytes)
		loop    .Loop
		jmp     FAILURE					; Our kernel image wasn't found in the root directory :-(

LOAD_FAT:
	mov     dx, WORD [di + 0x001A]		; Add 26 bytes to the current entry of the root directory, so that we get the start cluster
	mov     WORD [Cluster], dx          ; Store the 2 bytes of the start cluster (byte 26 & 27 of the root directory entry) in the variable "cluster"

	; Calculate the number of sectors used by all FATs (bpbNumberOfFATs * bpbSectorsPerFAT)
	xor     ax, ax
	mov		BYTE [Track], al			; Initialize the track with 0
	mov		BYTE [Head], al				; Initialize the head with 0
    mov     al, [bpbNumberOfFATs]		; The number of FATs
    mul     WORD [bpbSectorsPerFAT]		; The sectors per FAT
	mov		dh, al						; Store the number of sectors for all FATs in register DX

	; Load the FAT into memory
	mov		bx, ROOTDIRECTORY_AND_FAT_OFFSET		; Offset in memory at which we want to load the FATs
	mov		cx, WORD [bpbReservedSectors]			; Number of the reserved sectors (1)
	add		cx, 1									; Add 1 to the number of reserved sectors, so that our start sector is the 2nd sector (directly after the boot sector)
	mov		BYTE [Sector], cl						; Sector where we start to read
	call	disk_load								; Call the load routine

	mov		bx, IMAGE_OFFSET						; Address where the first cluster should be stored
	push	bx										; Store the current kernel address on the stack

LOAD_IMAGE:
    mov     ax, WORD [Cluster]						; FAT cluster to read
	call    FATCluster2LBA							; Convert the FAT cluster to LBA (result stored in AX)
	
	; Convert the calculated LBA address (input in AX) to a CHS address
	call	LBA2CHS

	xor		dx, dx
	mov     dh, BYTE [bpbSectorsPerCluster]			; Number of the sectors we want to read
	mov		bx, 0x2000
	mov		es, bx									; Set the Segment to 0x2000
	pop		bx										; Get the current kernel address from the stack (for every sector we read, we advance the address by 512 bytes)
	call    disk_load								; Read the cluster into memory
	add		bx, 0x200								; Advance the kernel address by 512 bytes (1 sector that was read from disk)

	push	bx

	; Compute the next cluster that we have to load from disk
	mov     ax, WORD [Cluster]						; identify current cluster
    mov     cx, ax									; copy current cluster
    mov     dx, ax									; copy current cluster
    shr     dx, 0x0001								; divide by two
    add     cx, dx									; sum for (3/2)
    mov     bx, ROOTDIRECTORY_AND_FAT_OFFSET        ; location of FAT in memory
    add     bx, cx									; index into FAT
    mov     dx, WORD [bx]							; read two bytes from FAT
    test    ax, 0x0001
    jnz     .ODD_CLUSTER
          
.EVEN_CLUSTER:
    and     dx, 0000111111111111b					; Take the lowest 12 bits
    jmp     .DONE
         
.ODD_CLUSTER:
	shr     dx, 0x0004								; Take the highest 12 bits
          
.DONE:
    mov     WORD [Cluster], dx						; store new cluster
	cmp     dx, 0x0FF0								; Test for end of file
    jb      LOAD_IMAGE