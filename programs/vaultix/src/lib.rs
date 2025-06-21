#![allow(unexpected_cfgs)]
#![allow(deprecated)]

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer, Burn, MintTo};
use anchor_spl::associated_token::AssociatedToken;
use pyth_sdk_solana::{load_price_feed_from_account_info};

declare_id!("6T71uU76fpMr6EBUJqjwT4T3wmuQpzj8QxnnEVv4T8cc");

#[program]
pub mod vaultix {

    use super::*;

    // Initialize vault
    pub fn init_vault(ctx: Context<InitVault>) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        vault_state.admin = ctx.accounts.admin.key();
        vault_state.collateral_vault = ctx.accounts.collateral_vault.key();
        vault_state.isol_token_mint = ctx.accounts.isol_token_mint.key();
        vault_state.total_deposited_sol = 0;
        vault_state.total_borrowed_sol = 0;
        vault_state.interest_rate = 7; // 7% interest rate for deposited SOL
        vault_state.debt_interest_rate = 10; // 10% interest rate for borrowed SOL
        vault_state.liquidation_threshold = 90; // or some % in basis points
        vault_state.bump = ctx.bumps.vault_state;

        emit!(CreateVault{
            admin: ctx.accounts.admin.key(),
            collateral_vault: ctx.accounts.collateral_vault.key(),
            isol_token_mint: ctx.accounts.isol_token_mint.key(),
            interest_rate: 7,
            debt_interest_rate: 10,
            liquidation_threshold: 90, 
        });

        Ok(())
    }

    // Initialize user position
    pub fn init_user_position(ctx: Context<InitUserPosition>) -> Result<()> {
        let user_position = &mut ctx.accounts.user_position;
        user_position.user = ctx.accounts.user.key();
        user_position.deposited_sol = 0;
        user_position.borrowed_sol = 0;
        user_position.collateralized_isol_tokens = 0;
        user_position.last_borrowed_timestamp = 0;
        user_position.bump = ctx.bumps.user_position;
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

        emit!(Deposit{
            user: user.key(),
            amount: amount,
        });

        Ok(())
    }

// pub fn add_interest(ctx: Context<AddInterest>) -> Result<()> {
//     add_interest_amount(
//         &ctx.accounts.vault_state,
//         &mut ctx.accounts.user_position,
//         &ctx.accounts.isol_token_mint.to_account_info(),
//         &mut ctx.accounts.user_isol_token_account,
//         &ctx.accounts.token_program,
//         &ctx.accounts.vault_state.to_account_info(),
//     )
// }


    // Add collateral from user to vault(escrow)
    pub fn add_collateral(ctx: Context<AddCollateral>, amount: u64) -> Result<()> {
        add_interest_amount(
        &ctx.accounts.vault_state,
        &mut ctx.accounts.user_position,
        &ctx.accounts.isol_token_mint.to_account_info(), 
        &mut ctx.accounts.user_isol_token_account,
        &ctx.accounts.token_program,
        &ctx.accounts.vault_state.to_account_info(),
    )?;

    let user_position = &mut ctx.accounts.user_position;
    let user_isol_token_account = &mut ctx.accounts.user_isol_token_account;
    let user = &ctx.accounts.user;

    require!(user_isol_token_account.amount >= user_position.collateralized_isol_tokens + amount, ErrorCode::InsufficientCollateral);

    // Transfer iSOL from user to vault's collateral vault
    let cpi_accounts = Transfer {
        from: user_isol_token_account.to_account_info(),
        to: ctx.accounts.collateral_vault.to_account_info(),
        authority: user.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    token::transfer(CpiContext::new(cpi_program, cpi_accounts), amount)?;
    // Increase the collateralized isol tokens
    user_position.collateralized_isol_tokens = user_position.collateralized_isol_tokens.checked_add(amount).unwrap();
  
    emit!(CollateralAdded{
        user: user.key(),
        amount: amount,
    });

    Ok(())
    }

    // Borrow SOL from vault
    pub fn borrow_sol(ctx: Context<BorrowSol>, amount: u64) -> Result<()> {

        add_interest_amount(
        &ctx.accounts.vault_state,
        &mut ctx.accounts.user_position,
        &ctx.accounts.isol_token_mint.to_account_info(), 
        &mut ctx.accounts.user_isol_token_account,
        &ctx.accounts.token_program,
        &ctx.accounts.vault_state.to_account_info(),
    )?;

        let user_position = &mut ctx.accounts.user_position;
        let admin_key = ctx.accounts.vault_state.admin;
            let vault_bump = ctx.accounts.vault_state.bump;

        // let user = &ctx.accounts.user;

        // Check if the user has enough collateral
        require!(user_position.collateralized_isol_tokens > 0, ErrorCode::InsufficientCollateral);
        require!(user_position.collateralized_isol_tokens >= amount * 100 / 75 as u64 , ErrorCode::InsufficientCollateral);

       // Transfer WSOL from vault to user
        let seeds = &[b"vault_state".as_ref(), admin_key.as_ref(), &[vault_bump]];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.collateral_vault.to_account_info(),
            to: ctx.accounts.user_wsol_account.to_account_info(),
            authority: ctx.accounts.vault_state.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(CpiContext::new_with_signer(cpi_program, cpi_accounts, signer), amount)?;

        // Update vault state
        let vault_state = &mut ctx.accounts.vault_state;
        vault_state.total_borrowed_sol = vault_state.total_borrowed_sol.checked_add(amount).unwrap();
        user_position.borrowed_sol = user_position.borrowed_sol.checked_add(amount).unwrap();
        user_position.last_borrowed_timestamp = Clock::get()?.unix_timestamp;

        emit!(BorrowedSol{
            user: user_position.user,
            amount: amount,
        });

        Ok(())
    }

    // Repay SOL to vault
    pub fn repay_sol(ctx: Context<RepaySol>, amount: u64) -> Result<()> {

        add_interest_amount(
        &ctx.accounts.vault_state,
        &mut ctx.accounts.user_position,
        &ctx.accounts.isol_token_mint.to_account_info(), 
        &mut ctx.accounts.user_isol_token_account,
        &ctx.accounts.token_program,
        &ctx.accounts.vault_state.to_account_info(),
    )?;

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

        emit!(Repayed{
            user: user.key(),
            amount: amount,
        });

        Ok(())
    }

    // Liquidate user's position. Will be called by crank or liquidator
    pub fn liquidate(ctx: Context<Liquidate>) -> Result<()> {

        add_interest_amount(
        &ctx.accounts.vault_state,
        &mut ctx.accounts.user_position,
        &ctx.accounts.isol_token_mint.to_account_info(), 
        &mut ctx.accounts.user_isol_token_account,
        &ctx.accounts.token_program,
        &ctx.accounts.vault_state.to_account_info(),
    )?;

        // To set the pyth feed to give the price of SOLUSD
        let expected_price_pubkey = "F7RyxQh3n1LDe5zHfjvE44aN7dXP3QWW3Gbn7b2ekVY6"
        .parse::<Pubkey>()
        .unwrap();

    require!(
        ctx.accounts.pyth_price_account.key() == expected_price_pubkey,
        ErrorCode::InvalidPriceFeed
    );

        let vault_state = &mut ctx.accounts.vault_state;
        let user_position = &mut ctx.accounts.user_position;

        require!(user_position.borrowed_sol > 0, ErrorCode::NothingToRepay);
        // let current_price = 100; // Placeholder for current price of SOL
        let interest_rate = vault_state.debt_interest_rate;
        let borrowed_sol = user_position.borrowed_sol;
        let collateralized_isol_tokens = user_position.collateralized_isol_tokens;
        let liquidation_threshold = vault_state.liquidation_threshold;
        
        // Calculate health factor
        let health_factor = get_health_factor(collateralized_isol_tokens, borrowed_sol, liquidation_threshold, interest_rate, &ctx.accounts.pyth_price_account)?;

        if health_factor < 1.0 {
            let amt_to_repay = (borrowed_sol as f64 * (1.0 + interest_rate as f64 / 100.0)) as u64; 
              
            // Update vault state
            user_position.collateralized_isol_tokens = user_position
                .collateralized_isol_tokens
                .checked_sub(amt_to_repay)
                .unwrap();

            user_position.borrowed_sol = user_position
                .borrowed_sol
                .checked_sub(amt_to_repay)
                .unwrap();
            
        emit!(Liquidated{
            user: user_position.user,
            amount: amt_to_repay ,
        });
        }

        

        Ok(())
    }

    // Withdraw deposited SOL from vault
   pub fn withdraw(ctx: Context<Withdraw>, amount: u64) -> Result<()> {

    add_interest_amount(
        &ctx.accounts.vault_state,
        &mut ctx.accounts.user_position,
        &ctx.accounts.isol_token_mint.to_account_info(), 
        &mut ctx.accounts.user_isol_token_account,
        &ctx.accounts.token_program,
        &ctx.accounts.vault_state.to_account_info(),
    )?;

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

  
    // Transfer WSOL from vault to user
    let seeds = &[b"vault_state".as_ref(), vault_state.admin.as_ref(), &[vault_state.bump]];
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

    emit!(Withdrew{
        user: user_position.user.key(),
        amount: amount,
    });

    Ok(())
}

}

#[derive(Accounts)]
pub struct InitVault<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + 32 + 32 + 32 + 8 + 8 + 8 + 8 + 8 + 1,
        seeds = [b"vault_state".as_ref(), admin.key().as_ref()],
        bump,
    )]
    pub vault_state: Account<'info, VaultState>,
    #[account(mut)]
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
    #[account(mut)]
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
    pub vault_state: Account<'info, VaultState>,
    #[account(mut)]
    pub isol_token_mint: Account<'info, Mint>, 
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub user_isol_token_account: Account<'info, TokenAccount>, // IB token account owned by user
    #[account(mut)]
    pub collateral_vault: Account<'info, TokenAccount>, // WSOL account owned by vault
    pub token_program: Program<'info, Token>,
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
    pub isol_token_mint: Account<'info, Mint>, // IB token mint
    #[account(mut)]
    pub user_isol_token_account: Account<'info, TokenAccount>, // IB token account owned by user
    #[account(mut)]
    pub user_wsol_account: Account<'info, TokenAccount>, // WSOL account owned
    #[account(mut)]
    pub collateral_vault: Account<'info, TokenAccount>, // WSOL account owned by vault
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Liquidate<'info> {
    #[account(mut)]
    pub vault_state: Account<'info, VaultState>,
    #[account(mut)]
    pub user_position: Account<'info, UserPosition>,
    #[account(mut)]
    pub isol_token_mint: Account<'info, Mint>, // IB token mint
    #[account(mut)]
    pub user_isol_token_account: Account<'info, TokenAccount>, // IB token account owned by user
    #[account(mut)]
    pub collateral_vault: Account<'info, TokenAccount>, // WSOL account owned by vault
    pub token_program: Program<'info, Token>,
    #[account(mut)]
    pub pyth_price_account: AccountInfo<'info>,
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
// space = 8 + 32 + 32 + 32 + 8 + 8 + 8 + 8 + 8 + 1
pub struct VaultState {
    pub admin: Pubkey,
    pub collateral_vault: Pubkey,
    pub isol_token_mint: Pubkey,
    pub total_deposited_sol: u64,
    pub total_borrowed_sol: u64,
    pub interest_rate: u64, // Interest rate for deposited SOL
    pub debt_interest_rate: u64, // Interest rate for borrowed SOL
    pub liquidation_threshold: u64, // Liquidation threshold for iSOL
    pub bump: u8,
}

#[account]
// space = 8 + 32 + 8 + 8 + 8 + 8 + 1
pub struct UserPosition {
    pub user: Pubkey,
    pub deposited_sol: u64, //This is the same as amount of iSOL as it is issued in 1:1 ratio
    pub borrowed_sol: u64,
    pub collateralized_isol_tokens: u64, //interest-bearing tokens
    pub last_borrowed_timestamp: i64,
    pub bump: u8,
}

#[error_code]
pub enum ErrorCode {
    #[msg("User has insufficient collateral")]
    InsufficientCollateral,
    #[msg("User has nothing to repay")]
    NothingToRepay,
    #[msg("Cannot withdraw collateral")]
    CannotWithdrawCollateral,
    #[msg("Invalid mint")]
    InvalidMint,
    #[msg("Invalid price feed")]
    InvalidPriceFeed,
}

#[event]
pub struct CreateVault{
    pub admin: Pubkey,
    pub collateral_vault: Pubkey,
    pub isol_token_mint: Pubkey,
    pub interest_rate: u64, 
    pub debt_interest_rate: u64, 
    pub liquidation_threshold: u64, 
}

#[event]
pub struct Deposit{
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct CollateralAdded{
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct BorrowedSol{
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct Repayed{
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct Liquidated{
    pub user: Pubkey,
    pub amount: u64, // Amount repaid
}

#[event]
pub struct Withdrew{
    pub user: Pubkey,
    pub amount: u64,
}

// Remember to make the vault_state as the authority for the IB token mint

// // Helper function to calculate health factor
pub fn get_health_factor(collateral: u64, borrowed: u64, liquidation_threshold: u64, interest_rate: u64, price_account_info: &AccountInfo) -> Result<f64> {
    
     // Fetch the current price of SOL from Pyth or any other oracle
    let now = Clock::get()?.unix_timestamp;
    let price_feed = load_price_feed_from_account_info( price_account_info ).unwrap();
    let current_price = price_feed.get_price_no_older_than(now, 60).unwrap();
    let current_price = (current_price.price) as u64;  


    let health_factor = (collateral as f64 *current_price as f64 * (liquidation_threshold as f64 /100.0)) /
    (borrowed as f64 * (current_price as f64) * (1.0 + interest_rate as f64 / 100.0));
    Ok(health_factor)}

// Helper function that calculates and adds interest and debt interest to user_position
pub fn add_interest_amount<'info>(
    vault_state: &VaultState,
    user_position: &mut UserPosition,
    isol_token_mint_info: &AccountInfo<'info>, // Change to AccountInfo instead of Pubkey
    user_isol_token_account: &mut Account<'info, TokenAccount>,
    token_program: &Program<'info, Token>,
    vault_state_account_info: &AccountInfo<'info>,
) -> Result<()> {
    // Verify the mint matches what's stored in vault_state
    require!(
        isol_token_mint_info.key() == vault_state.isol_token_mint,
        ErrorCode::InvalidMint
    );

    let last_timestamp = user_position.last_borrowed_timestamp;
    let now = Clock::get()?.unix_timestamp;
    let interest_rate = vault_state.interest_rate;
    let seconds_in_year = 365 * 24 * 60 * 60;
    let time_elapsed = (now - last_timestamp) as u64;
       
    let interest_earned = (user_position.deposited_sol as u128)
        .checked_mul(interest_rate as u128)
        .unwrap()
        .checked_mul(time_elapsed as u128)
        .unwrap()
        .checked_div(seconds_in_year as u128)
        .unwrap()
        .checked_div(100u128)
        .unwrap() as u64;

    // Create signer seeds
    let seeds = &[
        b"vault_state".as_ref(), 
        vault_state.admin.as_ref(), 
        &[vault_state.bump]
    ];
    let signer = &[&seeds[..]];
    
    // Mint interest tokens to user
    let cpi_accounts = MintTo {
        mint: isol_token_mint_info.clone(), // Use the passed AccountInfo
        to: user_isol_token_account.to_account_info(),
        authority: vault_state_account_info.clone(),
    };
    let cpi_program = token_program.to_account_info();
    let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
    
    token::mint_to(cpi_ctx, interest_earned)?;

    // Update user position
    user_position.last_borrowed_timestamp = now;
    user_position.deposited_sol = user_position
        .deposited_sol
        .checked_add(interest_earned)
        .unwrap();

    // Check for interest on borrowed SOL
    let borrowed_sol = user_position.borrowed_sol;
    if borrowed_sol != 0 {
    let debt_interest_rate = vault_state.debt_interest_rate;
    let debt_interest_earned = (borrowed_sol as u128)
        .checked_mul(debt_interest_rate as u128)
        .unwrap()
        .checked_mul(time_elapsed as u128)
        .unwrap()
        .checked_div(seconds_in_year as u128)
        .unwrap()
        .checked_div(100u128)
        .unwrap() as u64;

    // Add to borrowed SOL
    user_position.borrowed_sol = user_position
        .borrowed_sol
        .checked_add(debt_interest_earned).unwrap();
}
    Ok(())
}