#![allow(unexpected_cfgs)]
#![allow(deprecated)]

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer, Burn, MintTo};

declare_id!("6T71uU76fpMr6EBUJqjwT4T3wmuQpzj8QxnnEVv4T8cc");

#[program]
pub mod vaultix {
    use super::*;

    pub fn init_vault(ctx: Context<InitVault>) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        vault_state.admin = ctx.accounts.admin.key();
        vault_state.collateral_vault = ctx.accounts.collateral_vault.key();
        vault_state.isol_token_mint = ctx.accounts.isol_token_mint.key();
        vault_state.total_deposited_sol = 0;
        vault_state.total_borrowed_sol = 0;
        vault_state.interest_rate = 0;
        vault_state.bump = *ctx.bumps.get("vault_state").unwrap();
        Ok(())
    }

    pub fn init_user_position(ctx: Context<InitUserPosition>) -> Result<()> {
        let user_position = &mut ctx.accounts.user_position;
        user_position.user = ctx.accounts.user.key();
        user_position.deposited_sol = 0;
        user_position.borrowed_sol = 0;
        user_position.collateralized_isol_tokens = 0;
        user_position.last_borrowed_ts = 0;
        user_position.bump = *ctx.bumps.get("user_position").unwrap();
        Ok(())
    }

    pub fn deposit_sol(ctx: Context<DepositSol>, amount: u64) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        let user_position = &mut ctx.accounts.user_position;
        let user = &ctx.accounts.user;
        let collateral_vault = &mut ctx.accounts.collateral_vault;

        // Transfer SOL from user to vault's WSOL account
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_wsol_ata.to_account_info(),
            to: collateral_vault.to_account_info(),
            authority: user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        let seeds = &[b"vault_state".as_ref(), vault_state.admin.as_ref(), &[vault_state.bump]];
        let signer = &[&seeds[..]];

        // Mint IB tokens to user
        let cpi_accounts = MintTo {
            mint: ctx.accounts.isol_token_mint.to_account_info(),
            to: ctx.accounts.user_isol_token_account.to_account_info(),
            authority: vault_state.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
        token::mint_to(cpi_ctx, amount)?;

        // Update vault state
        vault_state.total_deposited_sol = vault_state.total_deposited_sol.checked_add(amount).unwrap();
        user_position.deposited_sol = user_position.deposited_sol.checked_add(amount).unwrap();
        Ok(())
    }

    // Think about adding collateral to an escrow account or a separate vault
    pub fn add_collateral(ctx: Context<AddCollateral>, amount: u64) -> Result<()> {
        let user_position = &mut ctx.accounts.user_position;
        let user_isol_token_account = &mut ctx.accounts.user_isol_token_account;

    require!(user_isol_token_account.amount >= user_position.collateralized_isol_tokens + amount, ErrorCode::InsufficientCollateral);

    // Increase the collateralized isol tokens
    user_position.collateralized_isol_tokens = user_position.collateralized_isol_tokens.checked_add(amount).unwrap();
    Ok(())
    }

    pub fn borrow_sol(ctx: Context<BorrowSol>, amount: u64) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        let user_position = &mut ctx.accounts.user_position;
        let user = &ctx.accounts.user;

        // Check if the user has enough collateral
        require!(user_position.collateralized_isol_tokens > 0, ErrorCode::InsufficientCollateral);
        require!(user_position.collateralized_isol_tokens * 90/100 as u64 >= amount , ErrorCode::InsufficientCollateral);

       // Transfer WSOL from vault to user
        let seeds = &[b"vault_state", &[vault_state.bump]];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.collateral_vault.to_account_info(),
            to: ctx.accounts.user_wsol_account.to_account_info(),
            authority: ctx.accounts.vault_state.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(CpiContext::new_with_signer(cpi_program, cpi_accounts, signer), amount)?;

        // Update vault state
        vault_state.total_borrowed_sol = vault_state.total_borrowed_sol.checked_add(amount).unwrap();
        user_position.borrowed_sol = user_position.borrowed_sol.checked_add(amount).unwrap();
        user_position.last_borrowed_ts = Clock::get()?.unix_timestamp;

        Ok(())
    }

    pub fn repay_sol(ctx: Context<RepaySol>, amount: u64) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        let user_position = &mut ctx.accounts.user_position;
        let user = &ctx.accounts.user;
        let user_wsol_account = &mut ctx.accounts.user_wsol_account;
        let collateral_vault = &mut ctx.accounts.collateral_vault;
        let token_program = &ctx.accounts.token_program;

        require!(user_position.borrowed_sol >= amount, ErrorCode::NothingToRepay);

        // Transfer WSOL from user to vault's WSOL account
        let cpi_accounts = Transfer {
            from: user_wsol_account.to_account_info(),
            to: collateral_vault.to_account_info(),
            authority: user.to_account_info(),
        };
        let cpi_program = token_program.to_account_info();
        token::transfer(CpiContext::new(cpi_program, cpi_accounts), amount)?;
       
        // Update vault state
        vault_state.total_borrowed_sol = vault_state.total_borrowed_sol.checked_sub(amount).unwrap();
        user_position.borrowed_sol = user_position.borrowed_sol.checked_sub(amount).unwrap();

        // Update user position
        Ok(())
    }

    // pub fn liquidate(ctx: Context<Liquidate>, user: Pubkey, amount: u64) -> Result<()> {}

   pub fn withdraw(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
    let vault_state = &mut ctx.accounts.vault_state;
    let user_position = &mut ctx.accounts.user_position;

    let isol_balance = ctx.accounts.user_isol_token_account.amount;
    require!(
        isol_balance >= user_position.collateralized_isol_tokens + amount,
        ErrorCode::CannotWithdrawCollateral
    );

    // Burn iSOL from user
    let burn_ctx = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        Burn {
            mint: ctx.accounts.isol_token_mint.to_account_info(),
            from: ctx.accounts.user_isol_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        },
    );
    token::burn(burn_ctx, amount)?;

    // Update user iSOL record
    user_position.isol_tokens = user_position
        .isol_tokens
        .checked_sub(amount)
        .unwrap();

    // Transfer WSOL from vault to user
    let seeds = &[b"vault_state", &[vault_state.bump]];
    let signer = &[&seeds[..]];
    let cpi_accounts = Transfer {
        from: ctx.accounts.collateral_vault.to_account_info(),
        to: ctx.accounts.user_wsol_account.to_account_info(),
        authority: vault_state.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    token::transfer(CpiContext::new_with_signer(cpi_program, cpi_accounts, signer), amount)?;

    // Update vault state
    vault_state.total_deposited_sol = vault_state
        .total_deposited_sol
        .checked_sub(amount)
        .unwrap();
    user_position.deposited_sol = user_position
        .deposited_sol
        .checked_sub(amount)
        .unwrap();

    Ok(())
}

}

#[derive(Accounts)]
pub struct InitVault<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + 32 + 32 + 32 + 8 + 8 + 8 + 1,
        seeds = [b"vault_state".as_ref(), admin.key().as_ref()],
        bump,
    )]
    pub vault_state: Account<'info, VaultState>,
    pub admin: Signer<'info>,
    pub collateral_vault: Account<'info, TokenAccount>,
    pub isol_token_mint: Account<'info, Mint>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitUserPosition<'info> {
    #[account(
        init,
        payer = user,
        space = 8 + 32 + 8 + 8 + 8 + 8 + 1,
        seeds = [b"user_position".as_ref(), user.key().as_ref()],
        bump,
    )]
    pub user_position: Account<'info, UserPosition>,
    pub user: Signer<'info>,
    pub system_program: Program<'info, System>, 
}

#[derive(Accounts)]
pub struct DepositSol<'info> {
    #[account(mut)]
    pub vault_state: Account<'info, VaultState>,

    #[account(mut)]
    pub user_position: Account<'info, UserPosition>,

    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub collateral_vault: Account<'info, TokenAccount>, // WSOL account owned by vault

    #[account(
    mut,
    associated_token::mint = wsol_mint,
    associated_token::authority = user
    )]
    pub user_wsol_ata: Account<'info, TokenAccount>,

    #[account(address = spl_token::native_mint::id())]
    pub wsol_mint: Account<'info, Mint>,

    #[account(mut)]
    pub isol_token_mint: Account<'info, Mint>,    // IB token mint

    #[account(mut)]
    pub user_isol_token_account: Account<'info, TokenAccount>, // IB token account owned by user

    pub associated_token_program: Program<'info, AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,

    pub token_program: Program<'info, Token>,   // Token program
}

#[derive(Accounts)]
pub struct AddCollateral<'info> {
    #[account(mut)]
    pub user_position: Account<'info, UserPosition>,
    #[account(mut)]
    pub user_isol_token_account: Account<'info, TokenAccount>, // IB token account owned by user
}

#[derive(Accounts)]
pub struct BorrowSol<'info> {
    #[account(mut)]
    pub vault_state: Account<'info, VaultState>,
    #[account(mut)]
    pub user_position: Account<'info, UserPosition>,
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub collateral_vault: Account<'info, TokenAccount>, // WSOL account owned by vault
    #[account(
    mut,
    associated_token::mint = wsol_mint,
    associated_token::authority = user
    )]
    pub user_wsol_account: Account<'info, TokenAccount>,
    #[account(address = spl_token::native_mint::id())]
    pub wsol_mint: Account<'info, Mint>,
    #[account(mut)]
    pub isol_token_mint: Account<'info, Mint>,    // IB token mint
    #[account(mut)]
    pub user_isol_token_account: Account<'info, TokenAccount>, // IB token account owned by user
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,
    pub token_program: Program<'info, Token>,   // Token program
}

#[derive(Accounts)]
pub struct RepaySol<'info> {
    #[account(mut)]
    pub vault_state: Account<'info, VaultState>,
    #[account(mut)]
    pub user_position: Account<'info, UserPosition>,
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub user_wsol_account: Account<'info, TokenAccount>, // WSOL account owned
    #[account(mut)]
    pub collateral_vault: Account<'info, TokenAccount>, // WSOL account owned by vault
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub vault_state: Account<'info, VaultState>,

    #[account(mut)]
    pub user_position: Account<'info, UserPosition>,

    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub user_isol_token_account: Account<'info, TokenAccount>, // iSOL

    #[account(mut)]
    pub isol_token_mint: Account<'info, Mint>,

    #[account(mut)]
    pub user_wsol_account: Account<'info, TokenAccount>, // WSOL

    #[account(mut)]
    pub collateral_vault: Account<'info, TokenAccount>, // vault WSOL

    pub token_program: Program<'info, Token>,
}


#[account]
// space = 8 + 32 + 32 + 32 + 8 + 8 + 8 + 1
pub struct VaultState {
    pub admin: Pubkey,
    pub collateral_vault: Pubkey,
    pub isol_token_mint: Pubkey,
    pub total_deposited_sol: u64,
    pub total_borrowed_sol: u64,
    pub interest_rate: u64,
    pub bump: u8,
}

#[account]
// space = 8 + 32 + 8 + 8 + 8 + 8 + 1
pub struct UserPosition {
    pub user: Pubkey,
    pub deposited_sol: u64,
    pub borrowed_sol: u64,
    pub collateralized_isol_tokens: u64, //interest-bearing tokens
    pub last_borrowed_ts: i64,
    pub bump: u8,
}


// Remember to make the vault_state as the authority for the IB token mint